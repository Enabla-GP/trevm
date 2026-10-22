//! Helpers for system actions including [EIP-4788], [EIP-6110], [EIP-7002] and
//! [EIP-7251].
//!
//! System actions are special state changes or smart contract calls made
//! before or after transaction exection. These actions are introduced via
//! hardfork. System actions are sometimes modeled as transactions with special
//! properties (as in [EIP-4788], [EIP-7002] and [EIP-7251]) or as special state
//! changes outside of the transaction lifecycle (as in [EIP-6110]).
//!
//! System transactions are modeled by the [`SystemTx`] struct, which implements
//! the [`Tx`] trait. The system transactions are sent from a special system
//! caller address: [`DEFAULT_SYSTEM_CALLER`]. Note that the system caller is
//! specified independently in each EIP, which allows introduction off
//! different system callers in future EIPs
//!
//! [`Tx`]: crate::Tx
//!
//! [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
//! [EIP-6110]: https://eips.ethereum.org/EIPS/eip-6110
//! [EIP-7002]: https://eips.ethereum.org/EIPS/eip-7002
//! [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251

mod fill;
pub use fill::{SystemTx, DEFAULT_SYSTEM_CALLER};

/// Helpers for Prague historical block hash [EIP-2935] system actions.
///
/// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
pub mod eip2935;

/// Helpers for Cancun beacon root [EIP-4788] system actions.
///
/// [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
pub mod eip4788;

/// Helpers for Cancun withdrawal [EIP-4895] system actions.
///
/// [EIP-4895]: https://eips.ethereum.org/EIPS/eip-4895
pub mod eip4895;

/// Helpers for Shanghai withdrawal [EIP-6110] system actions.
///
/// [EIP-6110]: https://eips.ethereum.org/EIPS/eip-6110
pub mod eip6110;

/// Helpers for Prague withdrawal request [EIP-7002] system actions.
///
/// [EIP-7002]: https://eips.ethereum.org/EIPS/eip-7002
pub mod eip7002;

/// Helpers for Prague consolidation request [EIP-7251] system actions.
///
/// [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251
pub mod eip7251;

/// The maximum blob gas limit for a block in Cancun.
pub const MAX_BLOB_GAS_PER_BLOCK_CANCUN: u64 = 786_432;

/// The maximum blob gas limit for a block in Prague.
pub const MAX_BLOB_GAS_PER_BLOCK_PRAGUE: u64 = 1_179_648;

/// The maximum blob gas limit for a block in Osaka, pre BPO-1.
pub const MAX_BLOB_GAS_PER_BLOCK_OSAKA: u64 = 1_749_648;

use crate::{
    helpers::{Ctx, Evm},
    EvmExtUnchecked, Tx,
};
use alloy::primitives::{Address, Bytes};
use revm::{
    bytecode::Bytecode,
    context::{
        result::{EVMError, ExecutionResult, ResultAndState},
        ContextTr, Transaction,
    },
    primitives::KECCAK_EMPTY,
    Database, DatabaseCommit, InspectSystemCallEvm, Inspector,
};

fn checked_insert_code<Db, Insp>(
    evm: &mut Evm<Db, Insp>,
    address: Address,
    code: &Bytes,
) -> Result<(), EVMError<Db::Error>>
where
    Db: Database + DatabaseCommit,
{
    if evm.account(address).map_err(EVMError::Database)?.info.code_hash == KECCAK_EMPTY {
        evm.set_bytecode(address, Bytecode::new_raw(code.clone())).map_err(EVMError::Database)?;
    }
    Ok(())
}

/// Clean up the system call, restoring the block env.
fn cleanup_syscall<Db, Insp>(
    evm: &mut Evm<Db, Insp>,
    result: &mut ResultAndState,
    syscall: &SystemTx,
    old_gas_limit: u64,
    old_base_fee: u64,
    previous_nonce_check: bool,
) where
    Db: Database + DatabaseCommit,
{
    // Restore the block environment
    evm.modify_block(|block| {
        block.gas_limit = old_gas_limit;
        block.basefee = old_base_fee;
    });

    // Restore the nonce check
    evm.ctx.modify_cfg(|cfg| cfg.disable_nonce_check = previous_nonce_check);

    // Native system calls do not charge gas or reward the beneficiary. Retain
    // every legitimate contract effect, including effects at the beneficiary.
    result.state.remove(&syscall.caller);
}

/// Extract successful protocol request bytes; failed post-ops invalidate the block.
fn request_output<DbError>(
    result: ExecutionResult,
    operation: &str,
) -> Result<Bytes, EVMError<DbError>> {
    match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data()),
        ExecutionResult::Revert { .. } => {
            Err(EVMError::Custom(format!("{operation} system call reverted")))
        }
        ExecutionResult::Halt { .. } => {
            Err(EVMError::Custom(format!("{operation} system call halted")))
        }
    }
}

/// Apply a system transaction as specified in [EIP-4788], [EIP-7002], or
/// [EIP-7251]. This function will execute the system transaction and apply
/// the result if non-error, cleaning up any extraneous state changes, and
/// restoring the block environment.
///
/// [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
/// [EIP-7002]: https://eips.ethereum.org/EIPS/eip-7002
/// [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251
pub(crate) fn execute_system_tx<Db, Insp>(
    evm: &mut Evm<Db, Insp>,
    syscall: &SystemTx,
) -> Result<ExecutionResult, EVMError<Db::Error>>
where
    Db: Database + DatabaseCommit,
    Insp: Inspector<Ctx<Db>>,
{
    syscall.fill_tx(evm);

    let limit = evm.tx().gas_limit();

    let block = &mut evm.ctx.block;

    let old_gas_limit = core::mem::replace(&mut block.gas_limit, limit);
    let old_base_fee = core::mem::take(&mut block.basefee);
    let previous_nonce_check = std::mem::replace(&mut evm.ctx.cfg.disable_nonce_check, true);

    let mut result =
        evm.inspect_system_call_with_caller(syscall.caller, syscall.target, syscall.input.clone())?;

    // Cleanup the syscall.
    cleanup_syscall(evm, &mut result, syscall, old_gas_limit, old_base_fee, previous_nonce_check);

    evm.ctx.db_mut().commit(result.state);

    // apply result, remove receipt from block outputs.
    Ok(result.result)
}

#[cfg(test)]
mod tests {
    use super::{
        eip4788::{
            eip4788_root_slot, eip4788_timestamp_slot, BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE,
        },
        eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
        eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
        request_output,
    };
    use crate::{NoopBlock, NoopCfg, TrevmBuilder};
    use alloy::primitives::{bytes, Address, Bytes, TxKind, B256, U256};
    use core::convert::Infallible;
    use revm::{
        bytecode::Bytecode,
        context::{
            result::{EVMError, ExecutionResult, HaltReason, Output, ResultGas, SuccessReason},
            BlockEnv, ContextTr, TxEnv,
        },
        database::InMemoryDB,
        primitives::hardfork::SpecId,
        state::{AccountInfo, EvmState},
        Database, DatabaseCommit,
    };

    #[test]
    fn canonical_effects_request_output_preserves_success_bytes() {
        let result = ExecutionResult::Success {
            reason: SuccessReason::Return,
            gas: ResultGas::default(),
            logs: Vec::new(),
            output: Output::Call(bytes!("010203")),
        };

        assert_eq!(request_output::<Infallible>(result, "EIP-7002").unwrap(), bytes!("010203"));
    }

    #[test]
    fn canonical_effects_request_output_rejects_revert_bytes() {
        let result = ExecutionResult::Revert {
            gas: ResultGas::default(),
            logs: Vec::new(),
            output: bytes!("010203"),
        };

        let error = request_output::<Infallible>(result, "EIP-7002").unwrap_err();
        assert!(matches!(error, EVMError::Custom(message)
            if message.contains("EIP-7002") && message.contains("reverted")));
    }

    #[test]
    fn canonical_effects_request_output_rejects_halt_without_panic() {
        let result = ExecutionResult::Halt {
            reason: HaltReason::InvalidFEOpcode,
            gas: ResultGas::default(),
            logs: Vec::new(),
        };

        let error = request_output::<Infallible>(result, "EIP-7251").unwrap_err();
        assert!(matches!(error, EVMError::Custom(message)
            if message.contains("EIP-7251") && message.contains("halted")));
    }

    #[test]
    fn system_beneficiary_eip4788_preserves_beacon_root_storage() {
        let timestamp = 8_u64;
        let root = B256::repeat_byte(0xaa);
        let mut trevm = crate::test_utils::test_trevm().fill_cfg(&NoopCfg).fill_block(
            &|block: &mut BlockEnv| {
                block.beneficiary = BEACON_ROOTS_ADDRESS;
                block.timestamp = U256::from(timestamp);
                block.gas_limit = 9_876_543;
                block.basefee = 17;
            },
        );
        trevm.test_set_bytecode(BEACON_ROOTS_ADDRESS, Bytecode::new_raw(BEACON_ROOTS_CODE.clone()));
        trevm.test_set_nonce(BEACON_ROOTS_ADDRESS, 7);
        trevm.test_set_balance(BEACON_ROOTS_ADDRESS, U256::from(19));

        trevm.apply_eip4788(root).unwrap();

        assert_eq!(
            trevm
                .try_read_storage(BEACON_ROOTS_ADDRESS, eip4788_timestamp_slot(timestamp))
                .unwrap(),
            U256::from(timestamp),
        );
        assert_eq!(
            trevm.try_read_storage(BEACON_ROOTS_ADDRESS, eip4788_root_slot(timestamp)).unwrap(),
            U256::from_be_bytes(root.0),
        );
        assert_eq!(trevm.try_read_balance(BEACON_ROOTS_ADDRESS).unwrap(), U256::from(19));
        assert_eq!(trevm.try_read_nonce(BEACON_ROOTS_ADDRESS).unwrap(), 7);
        assert_eq!(trevm.block().gas_limit, 9_876_543);
        assert_eq!(trevm.block().basefee, 17);
    }

    #[test]
    fn system_beneficiary_eip7002_preserves_real_queue_reset() {
        let user = Address::with_last_byte(0x42);
        let pubkey = [0x11_u8; 48];
        let amount = [0x22_u8; 8];
        let input = Bytes::from([pubkey.as_slice(), amount.as_slice()].concat());
        let mut trevm = crate::test_utils::test_trevm().fill_cfg(&NoopCfg).fill_block(&NoopBlock);
        trevm.apply_eip7002().unwrap();
        trevm.test_set_balance(user, U256::from(1_000));
        let transaction = |env: &mut TxEnv| {
            *env = TxEnv {
                caller: user,
                kind: TxKind::Call(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS),
                value: U256::ONE,
                gas_limit: 1_000_000,
                data: input.clone(),
                ..TxEnv::default()
            };
        };
        let transacted = trevm.run_tx(&transaction).unwrap();
        assert!(matches!(transacted.result(), ExecutionResult::Success { .. }));
        let mut trevm = transacted.accept_state();
        // EIP-7002 COUNT_STORAGE_SLOT is 1, in the actual deployed contract.
        assert_eq!(
            trevm.try_read_storage(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, U256::ONE).unwrap(),
            U256::ONE
        );
        trevm.inner_mut_unchecked().ctx.block.beneficiary = WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS;

        let requests = trevm.apply_eip7002().unwrap();

        assert_eq!(
            requests.as_ref(),
            [user.as_slice(), pubkey.as_slice(), amount.as_slice()].concat()
        );
        assert_eq!(
            trevm.try_read_storage(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, U256::ONE).unwrap(),
            U256::ZERO
        );
        assert!(
            trevm.apply_eip7002().unwrap().is_empty(),
            "the drained request cannot be emitted twice"
        );
    }

    #[test]
    fn system_beneficiary_eip7251_preserves_real_queue_reset() {
        let user = Address::with_last_byte(0x42);
        let source = [0x11_u8; 48];
        let target = [0x33_u8; 48];
        let input = Bytes::from([source.as_slice(), target.as_slice()].concat());
        let mut trevm = crate::test_utils::test_trevm().fill_cfg(&NoopCfg).fill_block(&NoopBlock);
        trevm.apply_eip7251().unwrap();
        trevm.test_set_balance(user, U256::from(1_000));
        let transaction = |env: &mut TxEnv| {
            *env = TxEnv {
                caller: user,
                kind: TxKind::Call(CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS),
                value: U256::ONE,
                gas_limit: 1_000_000,
                data: input.clone(),
                ..TxEnv::default()
            };
        };
        let transacted = trevm.run_tx(&transaction).unwrap();
        assert!(matches!(transacted.result(), ExecutionResult::Success { .. }));
        let mut trevm = transacted.accept_state();
        // EIP-7251 COUNT_STORAGE_SLOT is 1, in the actual deployed contract.
        assert_eq!(
            trevm.try_read_storage(CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, U256::ONE).unwrap(),
            U256::ONE
        );
        trevm.inner_mut_unchecked().ctx.block.beneficiary = CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS;

        let requests = trevm.apply_eip7251().unwrap();

        assert_eq!(
            requests.as_ref(),
            [user.as_slice(), source.as_slice(), target.as_slice()].concat()
        );
        assert_eq!(
            trevm.try_read_storage(CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, U256::ONE).unwrap(),
            U256::ZERO
        );
        assert!(
            trevm.apply_eip7251().unwrap().is_empty(),
            "the drained request cannot be emitted twice"
        );
    }

    #[test]
    fn system_beneficiary_zero_fee_suppression_preserves_unrelated_account() {
        let beneficiary = Address::with_last_byte(0x77);
        let mut trevm = crate::test_utils::test_trevm().fill_cfg(&NoopCfg).fill_block(
            &|block: &mut BlockEnv| {
                block.beneficiary = beneficiary;
                block.timestamp = U256::from(8);
                block.gas_limit = 9_876_543;
                block.basefee = 17;
            },
        );
        trevm.test_set_balance(beneficiary, U256::from(19));
        trevm.test_set_nonce(beneficiary, 7);

        trevm.apply_eip4788(B256::repeat_byte(0xaa)).unwrap();

        assert_eq!(trevm.try_read_balance(beneficiary).unwrap(), U256::from(19));
        assert_eq!(trevm.try_read_nonce(beneficiary).unwrap(), 7);
        assert_eq!(trevm.block().gas_limit, 9_876_543);
        assert_eq!(trevm.block().basefee, 17);
    }

    #[derive(Debug, Default)]
    struct BeneficiaryDb {
        inner: InMemoryDB,
        beneficiary: Address,
        touched: bool,
    }

    impl Database for BeneficiaryDb {
        type Error = Infallible;

        fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            self.inner.basic(address)
        }

        fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Self::Error> {
            self.inner.code_by_hash(hash)
        }

        fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
            self.inner.storage(address, slot)
        }

        fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
            self.inner.block_hash(number)
        }
    }

    impl DatabaseCommit for BeneficiaryDb {
        fn commit(&mut self, changes: EvmState) {
            self.touched |=
                changes.get(&self.beneficiary).is_some_and(|account| account.is_touched());
            self.inner.commit(changes);
        }
    }

    #[test]
    fn system_beneficiary_empty_account_is_not_artificially_touched() {
        let beneficiary = Address::with_last_byte(0x77);
        let mut database = BeneficiaryDb { beneficiary, ..BeneficiaryDb::default() };
        database.inner.insert_account_info(beneficiary, AccountInfo::default());
        assert!(database.inner.basic(beneficiary).unwrap().is_some());
        let mut trevm = TrevmBuilder::new()
            .with_spec_id(SpecId::PRAGUE)
            .with_db(database)
            .build_trevm()
            .fill_cfg(&NoopCfg)
            .fill_block(&|block: &mut BlockEnv| {
                block.beneficiary = beneficiary;
                block.timestamp = U256::from(8);
            });

        trevm.apply_eip4788(B256::repeat_byte(0xaa)).unwrap();

        assert!(
            !trevm.inner_mut_unchecked().ctx.db_mut().touched,
            "a zero-fee reward must not synthesize a touched empty beneficiary"
        );
    }
}
