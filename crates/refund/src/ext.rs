#[cfg(test)]
use mocktopus::macros::mockable;

#[cfg_attr(test, mockable)]
pub(crate) mod fee {
    use currency::Amount;
    use frame_support::dispatch::DispatchError;

    pub fn get_refund_fee_from_total<T: crate::Config>(amount: &Amount<T>) -> Result<Amount<T>, DispatchError> {
        <fee::Pallet<T>>::get_refund_fee_from_total(amount)
    }
}

#[cfg_attr(test, mockable)]
pub(crate) mod btc_relay {
    use bitcoin::types::{MerkleProof, Transaction, Value};
    use btc_relay::BtcAddress;
    use frame_support::dispatch::DispatchError;
    use sp_core::H256;
    use sp_std::convert::TryInto;

    pub fn verify_and_validate_op_return_transaction<T: crate::Config, V: TryInto<Value>>(
        merkle_proof: MerkleProof,
        transaction: Transaction,
        recipient_btc_address: BtcAddress,
        expected_btc: V,
        op_return_id: H256,
    ) -> Result<(), DispatchError> {
        <btc_relay::Pallet<T>>::verify_and_validate_op_return_transaction(
            merkle_proof,
            transaction,
            recipient_btc_address,
            expected_btc,
            op_return_id,
        )
    }

    pub fn parse_transaction<T: btc_relay::Config>(raw_tx: &[u8]) -> Result<Transaction, DispatchError> {
        <btc_relay::Pallet<T>>::parse_transaction(raw_tx)
    }

    pub fn parse_merkle_proof<T: btc_relay::Config>(raw_merkle_proof: &[u8]) -> Result<MerkleProof, DispatchError> {
        <btc_relay::Pallet<T>>::parse_merkle_proof(raw_merkle_proof)
    }
}

#[cfg_attr(test, mockable)]
pub(crate) mod security {
    use frame_support::dispatch::DispatchResult;
    use sp_core::H256;

    pub fn get_secure_id<T: crate::Config>(id: &T::AccountId) -> H256 {
        <security::Pallet<T>>::get_secure_id(id)
    }

    pub fn ensure_parachain_status_not_shutdown<T: crate::Config>() -> DispatchResult {
        <security::Pallet<T>>::ensure_parachain_status_not_shutdown()
    }
}

#[cfg_attr(test, mockable)]
pub(crate) mod vault_registry {
    use currency::Amount;
    use frame_support::dispatch::{DispatchError, DispatchResult};

    pub fn try_increase_to_be_issued_tokens<T: crate::Config>(
        vault_id: &T::AccountId,
        amount: &Amount<T>,
    ) -> Result<(), DispatchError> {
        <vault_registry::Pallet<T>>::try_increase_to_be_issued_tokens(vault_id, amount)
    }

    pub fn issue_tokens<T: crate::Config>(vault_id: &T::AccountId, amount: &Amount<T>) -> DispatchResult {
        <vault_registry::Pallet<T>>::issue_tokens(vault_id, amount)
    }
}
