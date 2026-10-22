use crate::{helpers::Ctx, EvmExtUnchecked, EvmNeedsTx};
use alloy::{eips::eip4895::Withdrawal, primitives::map::Entry};
use revm::{context::result::EVMError, state::EvmState, Database, DatabaseCommit, Inspector};

impl<Db, Insp> EvmNeedsTx<Db, Insp>
where
    Db: Database + DatabaseCommit,
    Insp: Inspector<Ctx<Db>>,
{
    /// Apply withdrawals, converting their Gwei amounts to wei.
    ///
    /// Repeated recipients accumulate in one pending account. State is committed
    /// once after all account reads succeed; a database error commits no credits.
    pub fn apply_withdrawals<'b>(
        &mut self,
        withdrawals: impl IntoIterator<Item = &'b Withdrawal>,
    ) -> Result<(), EVMError<Db::Error>> {
        // We need to apply the withdrawals by incrementing the balances of the
        // respective accounts, then committing the changes to the database.
        let mut changes = EvmState::default();

        let increments = withdrawals
            .into_iter()
            .map(|withdrawal| (withdrawal.address, withdrawal.amount_wei()))
            .filter(|(_, amount)| !amount.is_zero());

        for (address, amount) in increments {
            let acct = match changes.entry(address) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(
                    self.inner_mut_unchecked().account(address).map_err(EVMError::Database)?,
                ),
            };
            acct.info.balance = acct.info.balance.saturating_add(amount);
            acct.mark_touch();
        }

        self.commit_unchecked(changes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{NoopBlock, NoopCfg, TrevmBuilder};
    use alloy::{
        eips::eip4895::Withdrawal,
        primitives::{bytes, Address, B256, U256},
    };
    use revm::{
        bytecode::Bytecode,
        context::{result::EVMError, ContextTr},
        database::{DBErrorMarker, InMemoryDB},
        primitives::hardfork::SpecId,
        state::{AccountInfo, EvmState},
        Database, DatabaseCommit,
    };

    const USER: Address = Address::with_last_byte(0x42);
    const OTHER: Address = Address::with_last_byte(0x43);

    #[derive(Debug, PartialEq, Eq, thiserror::Error)]
    #[error("account read failed for {0}")]
    struct AccountReadError(Address);

    impl DBErrorMarker for AccountReadError {}

    #[derive(Debug, Default)]
    struct WithdrawalDb {
        inner: InMemoryDB,
        fail_at: Option<Address>,
        reads: Vec<Address>,
        commits: usize,
    }

    impl Database for WithdrawalDb {
        type Error = AccountReadError;

        fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            self.reads.push(address);
            if self.fail_at == Some(address) {
                return Err(AccountReadError(address));
            }
            Ok(self.inner.basic(address).unwrap())
        }

        fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Self::Error> {
            Ok(self.inner.code_by_hash(hash).unwrap())
        }

        fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
            Ok(self.inner.storage(address, slot).unwrap())
        }

        fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
            Ok(self.inner.block_hash(number).unwrap())
        }
    }

    impl DatabaseCommit for WithdrawalDb {
        fn commit(&mut self, changes: EvmState) {
            self.commits += 1;
            self.inner.commit(changes);
        }
    }

    const fn withdrawal(index: u64, address: Address, amount: u64) -> Withdrawal {
        Withdrawal { index, validator_index: index, address, amount }
    }

    #[test]
    fn canonical_effects_withdrawal_uses_wire_gwei() {
        let mut trevm = crate::test_utils::test_trevm().fill_cfg(&NoopCfg).fill_block(&NoopBlock);

        trevm.apply_withdrawals(&[withdrawal(1, USER, 1)]).unwrap();

        assert_eq!(trevm.try_read_balance(USER).unwrap(), U256::from(1_000_000_000));
    }

    #[test]
    fn canonical_effects_duplicate_withdrawals_preserve_account() {
        let code = Bytecode::new_raw(bytes!("6001600055"));
        let original = AccountInfo {
            balance: U256::from(7),
            nonce: 11,
            code_hash: code.hash_slow(),
            code: Some(code.clone()),
            ..Default::default()
        };
        let mut db = WithdrawalDb::default();
        db.inner.insert_account_info(USER, original.clone());
        db.inner.insert_account_storage(USER, U256::from(9), U256::from(42)).unwrap();
        let mut trevm = TrevmBuilder::new()
            .with_spec_id(SpecId::PRAGUE)
            .with_db(db)
            .build_trevm()
            .fill_cfg(&NoopCfg)
            .fill_block(&NoopBlock);

        trevm.apply_withdrawals(&[withdrawal(1, USER, 1), withdrawal(2, USER, 2)]).unwrap();

        let db = trevm.inner_mut_unchecked().ctx.db_mut();
        assert_eq!(db.reads, [USER]);
        assert_eq!(db.commits, 1);
        let account = db.inner.basic(USER).unwrap().unwrap();
        assert_eq!(account.balance, U256::from(3_000_000_007u64));
        assert_eq!(account.nonce, original.nonce);
        assert_eq!(account.code_hash, original.code_hash);
        assert_eq!(account.code, original.code);
        assert_eq!(db.inner.code_by_hash(account.code_hash).unwrap(), code);
        assert_eq!(db.inner.storage(USER, U256::from(9)).unwrap(), U256::from(42));
    }

    #[test]
    fn canonical_effects_later_read_error_does_not_commit_withdrawals() {
        let original = AccountInfo { balance: U256::from(7), nonce: 11, ..Default::default() };
        let mut db = WithdrawalDb { fail_at: Some(OTHER), ..Default::default() };
        db.inner.insert_account_info(USER, original.clone());
        let mut trevm = TrevmBuilder::new()
            .with_spec_id(SpecId::PRAGUE)
            .with_db(db)
            .build_trevm()
            .fill_cfg(&NoopCfg)
            .fill_block(&NoopBlock);

        let error = trevm
            .apply_withdrawals(&[withdrawal(1, USER, 1), withdrawal(2, OTHER, 2)])
            .unwrap_err();

        assert!(matches!(error, EVMError::Database(AccountReadError(OTHER))));
        let db = trevm.inner_mut_unchecked().ctx.db_mut();
        assert_eq!(db.reads, [USER, OTHER]);
        assert_eq!(db.commits, 0);
        assert_eq!(db.inner.basic(USER).unwrap().unwrap(), original);
        assert!(db.inner.basic(OTHER).unwrap().is_none());
    }
}
