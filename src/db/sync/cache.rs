//! The majority of this code has been reproduced from revm.

use alloy::primitives::{Address, B256};
use dashmap::DashMap;
use revm::{
    database::{
        states::{plain_account::PlainStorage, CacheAccount},
        CacheState, TransitionAccount,
    },
    state::{Account, AccountInfo, Bytecode, EvmState, EvmStorage},
};
use std::borrow::Cow;

/// A concurrent version of [`revm::database::CacheState`].
///
/// Most of the code for this has been reproduced from revm.
#[derive(Debug, Clone)]
pub struct ConcurrentCacheState {
    /// Block state account with account state.
    pub accounts: DashMap<Address, CacheAccount>,
    /// Created contracts.
    // TODO add bytecode counter for number of bytecodes added/removed.
    pub contracts: DashMap<B256, Bytecode>,
}

impl From<CacheState> for ConcurrentCacheState {
    fn from(other: CacheState) -> Self {
        Self {
            accounts: other.accounts.into_iter().collect(),
            contracts: other.contracts.into_iter().collect(),
        }
    }
}

impl Default for ConcurrentCacheState {
    fn default() -> Self {
        Self::new(true)
    }
}

impl ConcurrentCacheState {
    /// New default state.
    pub fn new(_has_state_clear: bool) -> Self {
        Self { accounts: DashMap::default(), contracts: DashMap::default() }
    }

    /// Absorb other into self, overwriting any existing values.
    pub fn absorb(&self, other: Self) {
        // NB: the `Extend` trait takes self by `&mut self`, so we have inlined
        // it here
        for pair in other.accounts.into_iter() {
            self.accounts.insert(pair.0, pair.1);
        }
        for pair in other.contracts.into_iter() {
            self.contracts.insert(pair.0, pair.1);
        }
    }

    /// Set state clear flag. EIP-161.
    pub const fn set_state_clear_flag(&mut self, _has_state_clear: bool) {}

    /// Insert not existing account.
    pub fn insert_not_existing(&self, address: Address) {
        self.accounts.insert(address, CacheAccount::new_loaded_not_existing());
    }

    /// Insert Loaded (Or LoadedEmptyEip161 if account is empty) account.
    pub fn insert_account(&self, address: Address, info: AccountInfo) {
        let account = if !info.is_empty() {
            CacheAccount::new_loaded(info, PlainStorage::default())
        } else {
            CacheAccount::new_loaded_empty_eip161(PlainStorage::default())
        };
        self.accounts.insert(address, account);
    }

    /// Similar to `insert_account` but with storage.
    pub fn insert_account_with_storage(
        &self,
        address: Address,
        info: AccountInfo,
        storage: PlainStorage,
    ) {
        let account = if !info.is_empty() {
            CacheAccount::new_loaded(info, storage)
        } else {
            CacheAccount::new_loaded_empty_eip161(storage)
        };
        self.accounts.insert(address, account);
    }

    /// Apply output of revm execution and create account transitions that are used to build BundleState.
    pub fn apply_evm_state(
        &self,
        evm_state: EvmState,
    ) -> Vec<(Address, TransitionAccount<Option<Cow<'static, EvmStorage>>>)> {
        let mut transitions = Vec::with_capacity(evm_state.len());
        for (address, account) in evm_state {
            if let Some(transition) = self.apply_account_state(address, Cow::Owned(account)) {
                transitions.push((address, transition));
            }
        }
        transitions
    }

    /// Apply updated account state to the cached account.
    /// Returns account transition if applicable.
    fn apply_account_state<'a>(
        &self,
        address: Address,
        account: Cow<'a, Account>,
    ) -> Option<TransitionAccount<Option<Cow<'a, EvmStorage>>>> {
        // not touched account are never changed.
        if !account.is_touched() {
            return None;
        }

        // The account may not be present in the cache when execution happened on top
        // of a different database. In that case, we insert account into the cache as if it was just loaded.
        let mut this_account = match self.accounts.entry(address) {
            dashmap::Entry::Occupied(entry) => entry.into_ref(),
            dashmap::Entry::Vacant(entry) => {
                let cache_account = if account.is_loaded_as_not_existing() {
                    CacheAccount::new_loaded_not_existing()
                } else {
                    let original = account.original_info();
                    if original.is_empty() {
                        CacheAccount::new_loaded_empty_eip161(PlainStorage::default())
                    } else {
                        CacheAccount::new_loaded(original, PlainStorage::default())
                    }
                };
                entry.insert(cache_account)
            }
        };

        // If it is marked as selfdestructed inside revm
        // we need to changed state to destroyed.
        if account.is_selfdestructed() {
            return this_account.selfdestruct();
        }

        let is_created = account.is_created();
        let is_empty = account.is_empty();

        // Note: it can happen that created contract get selfdestructed in same block
        // that is why is_created is checked after selfdestructed
        //
        // Note: Create2 opcode (Petersburg) was after state clear EIP (Spurious Dragon)
        //
        // Note: It is possibility to create KECCAK_EMPTY contract with some storage
        // by just setting storage inside CRATE constructor. Overlap of those contracts
        // is not possible because CREATE2 is introduced later.
        if is_created {
            let transition = this_account.newly_created(account);
            // Cache created code so it is readable before the transitions are merged.
            if let Some(info) = transition.info.as_ref() {
                if let Some(code) = info.code.as_ref() {
                    self.contracts.insert(info.code_hash, code.clone());
                }
            }
            return Some(transition);
        }

        // Account is touched, but not selfdestructed or newly created.
        // Account can be touched and not changed.
        // And when empty account is touched it needs to be removed from database.
        // EIP-161 state clear
        if is_empty {
            this_account.touch_empty_eip161()
        } else {
            Some(this_account.change(account))
        }
    }
}

// Some code above and documentation is adapted from the revm crate, and is
// reproduced here under the terms of the MIT license.
//
// MIT License
//
// Copyright (c) 2021-2024 draganrakita
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
