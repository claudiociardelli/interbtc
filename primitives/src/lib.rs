#![cfg_attr(not(feature = "std"), no_std)]

use bitcoin::{Address as BtcAddress, PublicKey as BtcPublicKey};
use bstringify::bstringify;
use codec::{Decode, Encode};
#[cfg(feature = "std")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};
pub use sp_core::H256;
pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;
use sp_runtime::{
    generic,
    traits::{BlakeTwo256, IdentifyAccount, Verify},
    FixedI128, FixedPointNumber, FixedU128, MultiSignature, RuntimeDebug,
};
use sp_std::{
    convert::{Into, TryFrom},
    prelude::*,
};

pub use bitcoin::types::H256Le;

pub trait TruncateFixedPointToInt: FixedPointNumber {
    /// take a fixed point number and turns it into the truncated inner representation,
    /// e.g. FixedU128(1.23) -> 1u128
    fn truncate_to_inner(&self) -> Option<<Self as FixedPointNumber>::Inner>;
}

impl TruncateFixedPointToInt for SignedFixedPoint {
    fn truncate_to_inner(&self) -> Option<Self::Inner> {
        self.into_inner().checked_div(SignedFixedPoint::accuracy())
    }
}

impl TruncateFixedPointToInt for UnsignedFixedPoint {
    fn truncate_to_inner(&self) -> Option<<Self as FixedPointNumber>::Inner> {
        self.into_inner().checked_div(UnsignedFixedPoint::accuracy())
    }
}

pub mod issue {
    use super::*;

    #[derive(Encode, Decode, Clone, PartialEq)]
    #[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
    pub enum IssueRequestStatus {
        /// opened, but not yet executed or cancelled
        Pending,
        /// payment was received, optional refund ID on overpayment (when vault cannot back)
        Completed(Option<H256>),
        /// payment was not received, vault may receive griefing collateral
        Cancelled,
    }

    impl Default for IssueRequestStatus {
        fn default() -> Self {
            IssueRequestStatus::Pending
        }
    }

    // Due to a known bug in serde we need to specify how u128 is (de)serialized.
    // See https://github.com/paritytech/substrate/issues/4641
    #[derive(Encode, Decode, Default, Clone, PartialEq)]
    #[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
    pub struct IssueRequest<AccountId, BlockNumber, Balance> {
        /// the vault associated with this issue request
        pub vault: AccountId,
        /// the *active* block height when this request was opened
        pub opentime: BlockNumber,
        /// the issue period when this request was opened
        pub period: BlockNumber,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// the collateral held for spam prevention
        pub griefing_collateral: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// the number of tokens that will be transferred to the user (as such, this does not include the fee)
        pub amount: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// the number of tokens that will be transferred to the fee pool
        pub fee: Balance,
        /// the account issuing tokens
        pub requester: AccountId,
        /// the vault's Bitcoin deposit address
        pub btc_address: BtcAddress,
        /// the vault's Bitcoin public key (when this request was made)
        pub btc_public_key: BtcPublicKey,
        /// the highest recorded height in the BTC-Relay (at time of opening)
        pub btc_height: u32,
        /// the status of this issue request
        pub status: IssueRequestStatus,
    }
}

#[cfg(feature = "std")]
fn serialize_as_string<S: Serializer, T: std::fmt::Display>(t: &T, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&t.to_string())
}

#[cfg(feature = "std")]
fn deserialize_from_string<'de, D: Deserializer<'de>, T: std::str::FromStr>(deserializer: D) -> Result<T, D::Error> {
    let s = String::deserialize(deserializer)?;
    s.parse::<T>()
        .map_err(|_| serde::de::Error::custom("Parse from string failed"))
}

pub mod redeem {
    use super::*;

    #[derive(Encode, Decode, Clone, Eq, PartialEq)]
    #[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
    pub enum RedeemRequestStatus {
        /// opened, but not yet executed or cancelled
        Pending,
        /// successfully executed with a valid payment from the vault
        Completed,
        /// bool=true indicates that the vault minted tokens for the amount that the redeemer burned
        Reimbursed(bool),
        /// user received compensation, but is retrying the redeem with another vault
        Retried,
    }

    impl Default for RedeemRequestStatus {
        fn default() -> Self {
            RedeemRequestStatus::Pending
        }
    }

    // Due to a known bug in serde we need to specify how u128 is (de)serialized.
    // See https://github.com/paritytech/substrate/issues/4641
    #[derive(Encode, Decode, Default, Clone, PartialEq)]
    #[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
    pub struct RedeemRequest<AccountId, BlockNumber, Balance> {
        /// the vault associated with this redeem request
        pub vault: AccountId,
        /// the *active* block height when this request was opened
        pub opentime: BlockNumber,
        /// the redeem period when this request was opened
        pub period: BlockNumber,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// total redeem fees - taken from request amount
        pub fee: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// amount the vault should spend on the bitcoin inclusion fee - taken from request amount
        pub transfer_fee_btc: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// total amount of BTC for the vault to send
        pub amount_btc: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// premium redeem amount in collateral
        pub premium: Balance,
        /// the account redeeming tokens (for BTC)
        pub redeemer: AccountId,
        /// the user's Bitcoin address for payment verification
        pub btc_address: BtcAddress,
        /// the highest recorded height in the BTC-Relay (at time of opening)
        pub btc_height: u32,
        /// the status of this redeem request
        pub status: RedeemRequestStatus,
    }
}

pub mod refund {
    use super::*;

    // Due to a known bug in serde we need to specify how u128 is (de)serialized.
    // See https://github.com/paritytech/substrate/issues/4641
    #[derive(Encode, Decode, Default, Clone, PartialEq)]
    #[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
    pub struct RefundRequest<AccountId, Balance> {
        /// the vault associated with this redeem request
        pub vault: AccountId,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// the total amount the vault should send
        pub amount_wrapped: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// total refund fees - taken from request amount
        pub fee: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// the total amount which was overpaid
        pub amount_btc: Balance,
        /// the account on issue which overpaid
        pub issuer: AccountId,
        /// the user's Bitcoin address for payment verification
        pub btc_address: BtcAddress,
        /// the corresponding issue request identifier
        pub issue_id: H256,
        /// whether the refund was executed or not
        pub completed: bool,
    }
}

pub mod replace {
    use super::*;

    #[derive(Encode, Decode, Clone, PartialEq)]
    #[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
    pub enum ReplaceRequestStatus {
        /// accepted, but not yet executed or cancelled
        Pending,
        /// successfully executed with a valid payment from the old vault
        Completed,
        /// payment was not received, new vault may receive griefing collateral
        Cancelled,
    }

    impl Default for ReplaceRequestStatus {
        fn default() -> Self {
            ReplaceRequestStatus::Pending
        }
    }
    // Due to a known bug in serde we need to specify how u128 is (de)serialized.
    // See https://github.com/paritytech/substrate/issues/4641
    #[derive(Encode, Decode, Default, Clone, PartialEq)]
    #[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
    pub struct ReplaceRequest<AccountId, BlockNumber, Balance> {
        /// the vault which has requested to be replaced
        pub old_vault: AccountId,
        /// the vault which is replacing the old vault
        pub new_vault: AccountId,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// the amount of tokens to be replaced
        pub amount: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// the collateral held for spam prevention
        pub griefing_collateral: Balance,
        #[cfg_attr(feature = "std", serde(bound(deserialize = "Balance: std::str::FromStr")))]
        #[cfg_attr(feature = "std", serde(deserialize_with = "deserialize_from_string"))]
        #[cfg_attr(feature = "std", serde(bound(serialize = "Balance: std::fmt::Display")))]
        #[cfg_attr(feature = "std", serde(serialize_with = "serialize_as_string"))]
        /// additional collateral to cover replacement
        pub collateral: Balance,
        /// the *active* block height when this request was opened
        pub accept_time: BlockNumber,
        /// the replace period when this request was opened
        pub period: BlockNumber,
        /// the Bitcoin address of the new vault
        pub btc_address: BtcAddress,
        /// the highest recorded height in the BTC-Relay (at time of opening)
        pub btc_height: u32,
        /// the status of this replace request
        pub status: ReplaceRequestStatus,
    }
}

pub mod oracle {
    use super::*;

    #[derive(Encode, Decode, Clone, Eq, PartialEq, Debug)]
    pub enum Key {
        ExchangeRate(CurrencyId),
        FeeEstimation,
    }
}

/// An index to a block.
pub type BlockNumber = u32;

/// Alias to 512-bit hash when used in the context of a transaction signature on the chain.
pub type Signature = MultiSignature;

/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// The type for looking up accounts. We don't expect more than 4 billion of them, but you
/// never know...
pub type AccountIndex = u32;

/// Index of a transaction in the chain. 32-bit should be plenty.
pub type Nonce = u32;

/// Balance of an account.
pub type Balance = u128;

/// Signed version of Balance
pub type Amount = i128;

/// Index of a transaction in the chain.
pub type Index = u32;

/// A hash of some data used by the chain.
pub type Hash = sp_core::H256;

/// An instant or duration in time.
pub type Moment = u64;

/// Digest item type.
pub type DigestItem = generic::DigestItem<Hash>;

/// Opaque block header type.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;

/// Opaque block type.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// Opaque block identifier type.
pub type BlockId = generic::BlockId<Block>;

/// The signed fixed point type.
pub type SignedFixedPoint = FixedI128;

/// The `Inner` type of the `SignedFixedPoint`.
pub type SignedInner = i128;

/// The unsigned fixed point type.
pub type UnsignedFixedPoint = FixedU128;

/// The `Inner` type of the `UnsignedFixedPoint`.
pub type UnsignedInner = u128;

pub trait CurrencyInfo {
    fn name(&self) -> &str;
    fn symbol(&self) -> &str;
    fn decimals(&self) -> u8;
}

macro_rules! create_currency_id {
    ($(#[$meta:meta])*
	$vis:vis enum CurrencyId {
        $($(#[$vmeta:meta])* $symbol:ident($name:expr, $deci:literal),)*
    }) => {
		$(#[$meta])*
		$vis enum CurrencyId {
			$($(#[$vmeta])* $symbol,)*
		}

        $(pub const $symbol: CurrencyId = CurrencyId::$symbol;)*

        impl CurrencyId {
			pub fn get_info() -> Vec<(&'static str, u32)> {
				vec![
					$((stringify!($symbol), $deci),)*
				]
			}

            pub const fn one(&self) -> Balance {
                10u128.pow(self.decimals() as u32)
            }

            const fn decimals(&self) -> u8 {
				match self {
					$(CurrencyId::$symbol => $deci,)*
				}
			}
		}

		impl CurrencyInfo for CurrencyId {
			fn name(&self) -> &str {
				match self {
					$(CurrencyId::$symbol => $name,)*
				}
			}
			fn symbol(&self) -> &str {
				match self {
					$(CurrencyId::$symbol => stringify!($symbol),)*
				}
			}
			fn decimals(&self) -> u8 {
				self.decimals()
			}
		}

		impl TryFrom<Vec<u8>> for CurrencyId {
			type Error = ();
			fn try_from(v: Vec<u8>) -> Result<CurrencyId, ()> {
				match v.as_slice() {
					$(bstringify!($symbol) => Ok(CurrencyId::$symbol),)*
					_ => Err(()),
				}
			}
		}
    }
}

create_currency_id! {
    #[derive(Encode, Decode, Eq, Hash, PartialEq, Copy, Clone, RuntimeDebug, PartialOrd, Ord)]
    #[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
    pub enum CurrencyId {
        DOT("Polkadot", 10),
        INTERBTC("interBTC", 8),
        INTR("Interlay", 10),

        KSM("Kusama", 12),
        KBTC("kinBTC", 8),
        KINT("Kintsugi", 12),
    }
}
