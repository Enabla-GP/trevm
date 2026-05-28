use alloy::primitives::{Address, U256};
use revm::{
    bytecode::Bytecode,
    database::{Database, DatabaseCommit, TryDatabaseCommit},
    state::{Account, AccountInfo, AccountStatus, EvmState, EvmStorageSlot, TransactionId},
};

fn transaction_id() -> TransactionId {
    TransactionId::new(0).expect("0 is a valid revm transaction id")
}

fn modified_account(
    original_info: AccountInfo,
    info: AccountInfo,
    storage: revm::state::EvmStorage,
) -> Account {
    let mut account = Account::default().with_info(original_info);
    account.set_current_info_as_original();
    account.info = info;
    account.storage = storage;
    account.status = AccountStatus::Touched;
    account.transaction_id = transaction_id();
    account
}

/// Test utilities for databases.
#[doc(hidden)]
pub trait DbTestExt: Database + DatabaseCommit {
    /// Modify an account via closure, creating it if it doesn't exist.
    fn test_modify_account<F>(&mut self, address: Address, f: F)
    where
        F: FnOnce(&mut AccountInfo),
    {
        let original_info = self.basic(address).ok().flatten().unwrap_or_default();
        let mut info = original_info.clone();
        f(&mut info);
        let account = modified_account(original_info, info, Default::default());
        self.commit(EvmState::from_iter([(address, account)]));
    }

    /// Set account balance.
    fn test_set_balance(&mut self, address: Address, balance: U256) {
        self.test_modify_account(address, |info| info.balance = balance);
    }

    /// Increase account balance (saturating).
    fn test_increase_balance(&mut self, address: Address, amount: U256) {
        self.test_modify_account(address, |info| {
            info.balance = info.balance.saturating_add(amount)
        });
    }

    /// Decrease account balance (saturating).
    fn test_decrease_balance(&mut self, address: Address, amount: U256) {
        self.test_modify_account(address, |info| {
            info.balance = info.balance.saturating_sub(amount)
        });
    }

    /// Set account nonce.
    fn test_set_nonce(&mut self, address: Address, nonce: u64) {
        self.test_modify_account(address, |info| info.nonce = nonce);
    }

    /// Set a storage slot.
    fn test_set_storage(&mut self, address: Address, slot: U256, value: U256) {
        let info = self.basic(address).ok().flatten().unwrap_or_default();
        let storage = [(slot, EvmStorageSlot::new_changed(U256::ZERO, value, transaction_id()))]
            .into_iter()
            .collect();
        let account = modified_account(info.clone(), info, storage);
        self.commit(EvmState::from_iter([(address, account)]));
    }

    /// Set account bytecode.
    fn test_set_bytecode(&mut self, address: Address, bytecode: Bytecode) {
        self.test_modify_account(address, |info| {
            info.code_hash = bytecode.hash_slow();
            info.code = Some(bytecode);
        });
    }
}

impl<Db: Database + DatabaseCommit> DbTestExt for Db {}

/// Test utilities for databases that support fallible commits.
#[doc(hidden)]
pub trait TryDbTestExt: Database + TryDatabaseCommit {
    /// Modify an account via closure, creating it if it doesn't exist.
    fn test_try_modify_account<F>(
        &mut self,
        address: Address,
        f: F,
    ) -> Result<(), <Self as TryDatabaseCommit>::Error>
    where
        F: FnOnce(&mut AccountInfo),
    {
        let original_info = self.basic(address).ok().flatten().unwrap_or_default();
        let mut info = original_info.clone();
        f(&mut info);
        let account = modified_account(original_info, info, Default::default());
        self.try_commit(EvmState::from_iter([(address, account)]))
    }

    /// Set account balance.
    fn test_try_set_balance(
        &mut self,
        address: Address,
        balance: U256,
    ) -> Result<(), <Self as TryDatabaseCommit>::Error> {
        self.test_try_modify_account(address, |info| info.balance = balance)
    }

    /// Increase account balance (saturating).
    fn test_try_increase_balance(
        &mut self,
        address: Address,
        amount: U256,
    ) -> Result<(), <Self as TryDatabaseCommit>::Error> {
        self.test_try_modify_account(address, |info| {
            info.balance = info.balance.saturating_add(amount)
        })
    }

    /// Decrease account balance (saturating).
    fn test_try_decrease_balance(
        &mut self,
        address: Address,
        amount: U256,
    ) -> Result<(), <Self as TryDatabaseCommit>::Error> {
        self.test_try_modify_account(address, |info| {
            info.balance = info.balance.saturating_sub(amount)
        })
    }

    /// Set account nonce.
    fn test_try_set_nonce(
        &mut self,
        address: Address,
        nonce: u64,
    ) -> Result<(), <Self as TryDatabaseCommit>::Error> {
        self.test_try_modify_account(address, |info| info.nonce = nonce)
    }

    /// Set a storage slot.
    fn test_try_set_storage(
        &mut self,
        address: Address,
        slot: U256,
        value: U256,
    ) -> Result<(), <Self as TryDatabaseCommit>::Error> {
        let info = self.basic(address).ok().flatten().unwrap_or_default();
        let storage = [(slot, EvmStorageSlot::new_changed(U256::ZERO, value, transaction_id()))]
            .into_iter()
            .collect();
        let account = modified_account(info.clone(), info, storage);
        self.try_commit(EvmState::from_iter([(address, account)]))
    }

    /// Set account bytecode.
    fn test_try_set_bytecode(
        &mut self,
        address: Address,
        bytecode: Bytecode,
    ) -> Result<(), <Self as TryDatabaseCommit>::Error> {
        self.test_try_modify_account(address, |info| {
            info.code_hash = bytecode.hash_slow();
            info.code = Some(bytecode);
        })
    }
}

impl<Db: Database + TryDatabaseCommit> TryDbTestExt for Db {}
