use super::{checked_insert_code, execute_system_tx, request_output};
use crate::{helpers::Ctx, system::SystemTx, EvmNeedsTx};
use alloy::{
    eips::eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_CODE,
    primitives::{Address, Bytes},
};
use revm::{
    context::result::EVMError, primitives::hardfork::SpecId, Database, DatabaseCommit, Inspector,
};

/// The address for the [EIP-7251] consolidation requests contract
///
/// [`EIP-7251`]: https://eips.ethereum.org/EIPS/eip-7251
pub use alloy::eips::eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS;

/// The size of a consolidation request in bytes.
pub const CONSOLIDATION_REQUEST_BYTES: usize = 20 + 48 + 48;

impl SystemTx {
    /// Instantiate a system call for the post-block consolidation requests as
    /// specified in [EIP-7251].
    ///
    /// [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251
    pub const fn eip7251() -> Self {
        Self::eip7251_with_target(CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS)
    }

    /// Instantiate a system call for the post-block consolidation requests as
    /// specified in [EIP-7251], with a custom target address.
    ///
    /// [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251
    pub const fn eip7251_with_target(target: Address) -> Self {
        Self::new(target, Bytes::new())
    }

    /// Instantiate a system call for the post-block consolidation requests as
    /// specified in [EIP-7251], with a custom target address and caller
    /// address.
    ///
    /// [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251
    pub const fn eip7251_with_target_and_caller(target: Address, caller: Address) -> Self {
        Self::new_with_caller(target, Bytes::new(), caller)
    }
}

impl<Db, Insp> EvmNeedsTx<Db, Insp>
where
    Db: Database + DatabaseCommit,
    Insp: Inspector<Ctx<Db>>,
{
    /// Apply a system transaction as specified in [EIP-7251]. The EIP-7251
    /// post-block action calls the consolidation request contract to process
    /// consolidation requests.
    ///
    /// Reverted or halted system calls return an operation-specific error; only
    /// successful calls produce request bytes.
    ///
    /// [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251
    pub fn apply_eip7251(&mut self) -> Result<Bytes, EVMError<Db::Error>> {
        if self.spec_id() < SpecId::PRAGUE {
            return Ok(Bytes::new());
        }

        checked_insert_code(
            self.inner_mut_unchecked(),
            CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
            &CONSOLIDATION_REQUEST_PREDEPLOY_CODE,
        )?;

        let res = execute_system_tx(self.inner_mut_unchecked(), &SystemTx::eip7251())?;

        request_output(res, "EIP-7251 consolidation request")
    }
}

#[cfg(test)]
mod tests {
    use super::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS;
    use crate::{NoopBlock, NoopCfg, Tx};
    use alloy::{
        consensus::constants::ETH_TO_WEI,
        primitives::{bytes, fixed_bytes, Address, Bytes, FixedBytes, TxKind, U256},
    };
    use revm::{
        bytecode::Bytecode,
        context::{result::EVMError, TxEnv},
    };

    const WITHDRAWAL_ADDR: Address = Address::with_last_byte(0x42);
    const VALIDATOR_PUBKEY: FixedBytes<48> = fixed_bytes!("111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111");
    const TARGET_VALIDATOR_PUBKEY: FixedBytes<48> = fixed_bytes!("111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111");

    struct ConsolidationTx;

    impl Tx for ConsolidationTx {
        fn fill_tx_env(&self, tx_env: &mut TxEnv) {
            let input: Bytes =
                [&VALIDATOR_PUBKEY[..], &TARGET_VALIDATOR_PUBKEY[..]].concat().into();

            tx_env.caller = WITHDRAWAL_ADDR;
            tx_env.data = input;
            tx_env.kind = TxKind::Call(CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS);
            // `MIN_CONSOLIDATION_REQUEST_FEE`
            tx_env.value = U256::from(1);
        }
    }

    #[test]
    fn test_eip7251() {
        let mut trevm = crate::test_utils::test_trevm().fill_cfg(&NoopCfg).fill_block(&NoopBlock);

        // insert the code
        trevm.apply_eip7251().unwrap();

        trevm.test_increase_balance(WITHDRAWAL_ADDR, U256::from(100 * ETH_TO_WEI));

        let mut trevm = trevm.run_tx(&ConsolidationTx).unwrap().accept_state();

        let requests = trevm.apply_eip7251().unwrap();

        assert_eq!(&requests[..20], WITHDRAWAL_ADDR.as_slice());
        assert_eq!(&requests[20..68], VALIDATOR_PUBKEY.as_slice());
        assert_eq!(&requests[68..], TARGET_VALIDATOR_PUBKEY.as_slice());
    }
    #[test]
    fn canonical_effects_eip7251_rejects_halting_predeploy() {
        let mut trevm = crate::test_utils::test_trevm().fill_cfg(&NoopCfg).fill_block(&NoopBlock);
        trevm.test_set_bytecode(
            CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
            Bytecode::new_raw(bytes!("fe")),
        );

        let error = trevm.apply_eip7251().unwrap_err();

        assert!(matches!(error, EVMError::Custom(message)
            if message.contains("EIP-7251") && message.contains("halted")));
    }
}
