//! # Vault Registry Module
//! Based on the [specification](https://interlay.gitlab.io/polkabtc-spec/spec/vaultregistry.html).

#![deny(warnings)]
#![cfg_attr(test, feature(proc_macro_hygiene))]
#![cfg_attr(not(feature = "std"), no_std)]

mod ext;
pub mod types;

#[cfg(any(feature = "runtime-benchmarks", test))]
mod benchmarking;

mod default_weights;

pub use default_weights::WeightInfo;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mock;

#[cfg(test)]
extern crate mocktopus;

#[cfg(test)]
use mocktopus::macros::mockable;

use crate::types::{
    BalanceOf, BtcAddress, Collateral, CurrencyId, DefaultSystemVault, RichSystemVault, RichVault, SignedFixedPoint,
    SignedInner, UnsignedFixedPoint, UpdatableVault, Version,
};

#[doc(inline)]
pub use crate::types::{BtcPublicKey, CurrencySource, DefaultVault, SystemVault, Vault, VaultStatus, Wallet};
use bitcoin::types::Value;
use codec::FullCodec;
pub use currency::Amount;
use frame_support::{
    dispatch::{DispatchError, DispatchResult},
    ensure,
    traits::Get,
    transactional, PalletId,
};
use frame_system::{
    ensure_signed,
    offchain::{SendTransactionTypes, SubmitTransaction},
};
use sp_core::{H256, U256};
#[cfg(feature = "std")]
use sp_runtime::traits::AtLeast32BitUnsigned;
use sp_runtime::{
    traits::*,
    transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidity, ValidTransaction},
    FixedPointNumber, FixedPointOperand,
};
use sp_std::{
    convert::{TryFrom, TryInto},
    fmt::Debug,
    vec::Vec,
};

// value taken from https://github.com/substrate-developer-hub/recipes/blob/master/pallets/ocw-demo/src/lib.rs
pub const UNSIGNED_TXS_PRIORITY: u64 = 100;

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;

    #[pallet::pallet]
    #[pallet::generate_store(trait Store)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config:
        frame_system::Config
        + SendTransactionTypes<Call<Self>>
        + oracle::Config
        + security::Config
        + staking::Config<
            SignedInner = SignedInner<Self>,
            SignedFixedPoint = SignedFixedPoint<Self>,
            CurrencyId = primitives::CurrencyId,
        > + reward::Config<SignedFixedPoint = SignedFixedPoint<Self>, CurrencyId = CurrencyId<Self>>
        + fee::Config<UnsignedInner = BalanceOf<Self>>
        + currency::Config<
            Balance = BalanceOf<Self>,
            CurrencyId = CurrencyId<Self>,
            SignedInner = <Self as fee::Config>::SignedInner,
        >
    {
        /// The vault module id, used for deriving its sovereign account ID.
        #[pallet::constant] // put the constant in metadata
        type PalletId: Get<PalletId>;

        /// The overarching event type.
        type Event: From<Event<Self>>
            + Into<<Self as frame_system::Config>::Event>
            + IsType<<Self as frame_system::Config>::Event>;

        /// The primitive balance type.
        type Balance: AtLeast32BitUnsigned
            + FixedPointOperand
            + Into<U256>
            + TryFrom<U256>
            + TryFrom<Value>
            + TryInto<Value>
            + MaybeSerializeDeserialize
            + FullCodec
            + Copy
            + Default
            + Debug;

        /// Weight information for the extrinsics in this module.
        type WeightInfo: WeightInfo;

        /// Currency used for griefing collateral, e.g. DOT.
        #[pallet::constant]
        type GetGriefingCollateralCurrencyId: Get<CurrencyId<Self>>;
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn offchain_worker(n: T::BlockNumber) {
            log::info!("Off-chain worker started on block {:?}", n);
            Self::_offchain_worker();
        }
    }

    #[pallet::validate_unsigned]
    impl<T: Config> frame_support::unsigned::ValidateUnsigned for Pallet<T> {
        type Call = Call<T>;

        fn validate_unsigned(source: TransactionSource, call: &Self::Call) -> TransactionValidity {
            match source {
                TransactionSource::External => {
                    // receiving unsigned transaction from network - disallow
                    return InvalidTransaction::Call.into();
                }
                TransactionSource::Local => {}   // produced by off-chain worker
                TransactionSource::InBlock => {} // some other node included it in a block
            };

            let valid_tx = |provide| {
                ValidTransaction::with_tag_prefix("vault-registry")
                    .priority(UNSIGNED_TXS_PRIORITY)
                    .and_provides([&provide])
                    .longevity(3)
                    .propagate(false)
                    .build()
            };

            match call {
                Call::report_undercollateralized_vault(_) => valid_tx(b"report_undercollateralized_vault".to_vec()),
                _ => InvalidTransaction::Call.into(),
            }
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Initiates the registration procedure for a new Vault.
        /// The Vault provides its BTC address and locks up collateral,
        /// which is to be used in the issuing process.
        ///
        /// # Arguments
        /// * `collateral` - the amount of collateral to lock
        /// * `public_key` - the BTC public key of the vault to register
        ///
        /// # Errors
        /// * `InsufficientVaultCollateralAmount` - if the collateral is below the minimum threshold
        /// * `VaultAlreadyRegistered` - if a vault is already registered for the origin account
        /// * `InsufficientCollateralAvailable` - if the vault does not own enough collateral
        #[pallet::weight(<T as Config>::WeightInfo::register_vault())]
        #[transactional]
        pub fn register_vault(
            origin: OriginFor<T>,
            #[pallet::compact] collateral: Collateral<T>,
            public_key: BtcPublicKey,
            currency_id: CurrencyId<T>,
        ) -> DispatchResultWithPostInfo {
            ext::security::ensure_parachain_status_not_shutdown::<T>()?;
            Self::_register_vault(&ensure_signed(origin)?, collateral, public_key, currency_id)?;
            Ok(().into())
        }

        /// Deposit collateral as a security against stealing the
        /// Bitcoin locked with the caller.
        ///
        /// # Arguments
        /// * `amount` - the amount of extra collateral to lock
        #[pallet::weight(<T as Config>::WeightInfo::deposit_collateral())]
        #[transactional]
        pub fn deposit_collateral(
            origin: OriginFor<T>,
            #[pallet::compact] amount: Collateral<T>,
        ) -> DispatchResultWithPostInfo {
            let sender = ensure_signed(origin)?;

            ext::security::ensure_parachain_status_not_shutdown::<T>()?;

            let vault = Self::get_active_rich_vault_from_id(&sender)?;

            let amount = Amount::new(amount, vault.data.currency_id);

            Self::try_deposit_collateral(&sender, &amount)?;

            Self::deposit_event(Event::<T>::DepositCollateral(
                vault.id(),
                amount.amount(),
                vault.get_total_collateral()?.amount(),
                vault.get_free_collateral()?.amount(),
            ));
            Ok(().into())
        }

        /// Withdraws `amount` of the collateral from the amount locked by
        /// the vault corresponding to the origin account
        /// The collateral left after withdrawal must be more
        /// (free or used in collateral issued tokens) than MinimumCollateralVault
        /// and above the SecureCollateralThreshold. Collateral that is currently
        /// being used to back issued tokens remains locked until the Vault
        /// is used for a redeem request (full release can take multiple redeem requests).
        ///
        /// # Arguments
        /// * `amount` - the amount of collateral to withdraw
        ///
        /// # Errors
        /// * `VaultNotFound` - if no vault exists for the origin account
        /// * `InsufficientCollateralAvailable` - if the vault does not own enough collateral
        #[pallet::weight(<T as Config>::WeightInfo::withdraw_collateral())]
        #[transactional]
        pub fn withdraw_collateral(
            origin: OriginFor<T>,
            #[pallet::compact] amount: Collateral<T>,
        ) -> DispatchResultWithPostInfo {
            let sender = ensure_signed(origin)?;
            ext::security::ensure_parachain_status_not_shutdown::<T>()?;

            let vault = Self::get_rich_vault_from_id(&sender)?;

            let amount = Amount::new(amount, vault.data.currency_id);
            Self::try_withdraw_collateral(&sender, &amount)?;

            Self::deposit_event(Event::<T>::WithdrawCollateral(
                sender,
                amount.amount(),
                vault.get_total_collateral()?.amount(),
            ));
            Ok(().into())
        }

        /// Registers a new Bitcoin address for the vault.
        ///
        /// # Arguments
        /// * `public_key` - the BTC public key of the vault to update
        #[pallet::weight(<T as Config>::WeightInfo::update_public_key())]
        #[transactional]
        pub fn update_public_key(origin: OriginFor<T>, public_key: BtcPublicKey) -> DispatchResultWithPostInfo {
            let account_id = ensure_signed(origin)?;
            ext::security::ensure_parachain_status_not_shutdown::<T>()?;
            let mut vault = Self::get_active_rich_vault_from_id(&account_id)?;
            vault.update_public_key(public_key.clone());
            Self::deposit_event(Event::<T>::UpdatePublicKey(account_id, public_key));
            Ok(().into())
        }

        #[pallet::weight(<T as Config>::WeightInfo::register_address())]
        #[transactional]
        pub fn register_address(origin: OriginFor<T>, btc_address: BtcAddress) -> DispatchResultWithPostInfo {
            let account_id = ensure_signed(origin)?;
            ext::security::ensure_parachain_status_not_shutdown::<T>()?;
            Self::insert_vault_deposit_address(&account_id, btc_address)?;
            Self::deposit_event(Event::<T>::RegisterAddress(account_id, btc_address));
            Ok(().into())
        }

        /// Configures whether or not the vault accepts new issues.
        ///
        /// # Arguments
        ///
        /// * `origin` - sender of the transaction (i.e. the vault)
        /// * `accept_new_issues` - true indicates that the vault accepts new issues
        ///
        /// # Weight: `O(1)`
        #[pallet::weight(<T as Config>::WeightInfo::accept_new_issues())]
        #[transactional]
        pub fn accept_new_issues(origin: OriginFor<T>, accept_new_issues: bool) -> DispatchResultWithPostInfo {
            ext::security::ensure_parachain_status_not_shutdown::<T>()?;
            let vault_id = ensure_signed(origin)?;
            let mut vault = Self::get_active_rich_vault_from_id(&vault_id)?;
            vault.set_accept_new_issues(accept_new_issues)?;
            Ok(().into())
        }

        #[pallet::weight(<T as Config>::WeightInfo::report_undercollateralized_vault())]
        #[transactional]
        pub fn report_undercollateralized_vault(
            _origin: OriginFor<T>,
            vault_id: T::AccountId,
        ) -> DispatchResultWithPostInfo {
            log::info!("Vault reported");
            let vault = Self::get_vault_from_id(&vault_id)?;
            let liquidation_threshold =
                Self::liquidation_collateral_threshold(vault.currency_id).ok_or(Error::<T>::ThresholdNotSet)?;
            if Self::is_vault_below_liquidation_threshold(&vault, liquidation_threshold)? {
                Self::liquidate_vault(&vault_id)?;
                Ok(().into())
            } else {
                log::info!("Not liquidating; vault not below liquidation threshold");
                Err(Error::<T>::VaultNotBelowLiquidationThreshold.into())
            }
        }

        /// Changes the collateral ceiling for a currency (only executable by the Root account)
        ///
        /// # Arguments
        /// * `currency_id` - the currency to change
        /// * `ceiling` - the new collateral ceiling
        #[pallet::weight(<T as Config>::WeightInfo::adjust_collateral_ceiling())]
        #[transactional]
        pub fn adjust_collateral_ceiling(
            origin: OriginFor<T>,
            currency_id: CurrencyId<T>,
            ceiling: Collateral<T>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            Self::set_collateral_ceiling(currency_id, ceiling);
            Ok(())
        }

        /// Changes the secure threshold for a currency (only executable by the Root account)
        ///
        /// # Arguments
        /// * `currency_id` - the currency to change
        /// * `threshold` - the new secure threshold
        #[pallet::weight(<T as Config>::WeightInfo::adjust_secure_collateral_threshold())]
        #[transactional]
        pub fn adjust_secure_collateral_threshold(
            origin: OriginFor<T>,
            currency_id: CurrencyId<T>,
            threshold: UnsignedFixedPoint<T>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            Self::set_secure_collateral_threshold(currency_id, threshold);
            Ok(())
        }

        /// Changes the collateral premium redeem threshold for a currency (only executable by the Root account)
        ///
        /// # Arguments
        /// * `currency_id` - the currency to change
        /// * `ceiling` - the new collateral ceiling
        #[pallet::weight(<T as Config>::WeightInfo::adjust_premium_redeem_threshold())]
        #[transactional]
        pub fn adjust_premium_redeem_threshold(
            origin: OriginFor<T>,
            currency_id: CurrencyId<T>,
            threshold: UnsignedFixedPoint<T>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            Self::set_premium_redeem_threshold(currency_id, threshold);
            Ok(())
        }

        /// Changes the collateral liquidation threshold for a currency (only executable by the Root account)
        ///
        /// # Arguments
        /// * `currency_id` - the currency to change
        /// * `ceiling` - the new collateral ceiling
        #[pallet::weight(<T as Config>::WeightInfo::adjust_liquidation_collateral_threshold())]
        #[transactional]
        pub fn adjust_liquidation_collateral_threshold(
            origin: OriginFor<T>,
            currency_id: CurrencyId<T>,
            threshold: UnsignedFixedPoint<T>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            Self::set_liquidation_collateral_threshold(currency_id, threshold);
            Ok(())
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    #[pallet::metadata(T::AccountId = "AccountId", T::BlockNumber = "BlockNumber", BalanceOf<T> = "Balance", CurrencyId<T> = "CurrencyId")]
    pub enum Event<T: Config> {
        /// vault_id, collateral, currency_id
        RegisterVault(T::AccountId, BalanceOf<T>, CurrencyId<T>),
        /// vault_id, new collateral, total collateral, free collateral
        DepositCollateral(T::AccountId, BalanceOf<T>, BalanceOf<T>, BalanceOf<T>),
        /// vault_id, withdrawn collateral, total collateral
        WithdrawCollateral(T::AccountId, BalanceOf<T>, BalanceOf<T>),
        /// vault_id, new public key
        UpdatePublicKey(T::AccountId, BtcPublicKey),
        /// vault_id, new address
        RegisterAddress(T::AccountId, BtcAddress),
        /// vault_id, additional to-be-issued tokens
        IncreaseToBeIssuedTokens(T::AccountId, BalanceOf<T>),
        /// vault_id, decrease in to-be-issued tokens
        DecreaseToBeIssuedTokens(T::AccountId, BalanceOf<T>),
        /// vault_id, additional number of issued tokens
        IssueTokens(T::AccountId, BalanceOf<T>),
        /// vault_id, additional to-be-redeemed tokens
        IncreaseToBeRedeemedTokens(T::AccountId, BalanceOf<T>),
        /// vault_id, decrease in to-be-redeemed tokens
        DecreaseToBeRedeemedTokens(T::AccountId, BalanceOf<T>),
        /// vault_id, additional to-be-replaced tokens
        IncreaseToBeReplacedTokens(T::AccountId, BalanceOf<T>),
        /// vault_id, decrease in to-be-replaced tokens
        DecreaseToBeReplacedTokens(T::AccountId, BalanceOf<T>),
        /// vault_id, user_id, amount of tokens reduced in issued & to-be-redeemed
        DecreaseTokens(T::AccountId, T::AccountId, BalanceOf<T>),
        /// vault_id, amount of newly redeemed tokens
        RedeemTokens(T::AccountId, BalanceOf<T>),
        /// vault_id, amount of newly redeemed tokens, amount of collateral transferred, user_id
        RedeemTokensPremium(T::AccountId, BalanceOf<T>, BalanceOf<T>, T::AccountId),
        /// vault_id, amount of newly redeemed tokens, slashed collateral
        RedeemTokensLiquidatedVault(T::AccountId, BalanceOf<T>, BalanceOf<T>),
        /// vault_id, amount of burned tokens, transferred collateral
        RedeemTokensLiquidation(T::AccountId, BalanceOf<T>, BalanceOf<T>),
        /// old_vault_id, new_vault_id, transferred tokens, additional collateral locked by new_vault
        ReplaceTokens(T::AccountId, T::AccountId, BalanceOf<T>, BalanceOf<T>),
        /// vault_id, issued_tokens, to_be_issued_tokens, to_be_redeemed_tokens,
        /// to_be_replaced_tokens, backing_collateral, status, replace_collateral
        LiquidateVault(
            T::AccountId, // vault_id
            BalanceOf<T>, // issued_tokens
            BalanceOf<T>, // to_be_issued_tokens
            BalanceOf<T>, // to_be_redeemed_tokens
            BalanceOf<T>, // to_be_replaced_tokens
            BalanceOf<T>, // backing_collateral
            VaultStatus,  // status
            BalanceOf<T>, // replace_collateral
        ),
        /// vault_id, banned_until
        BanVault(T::AccountId, T::BlockNumber),
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Not enough free collateral available.
        InsufficientCollateral,
        /// The amount of tokens to be issued is higher than the issuable amount by the vault
        ExceedingVaultLimit,
        /// The requested amount of tokens exceeds the amount available to this vault.
        InsufficientTokensCommitted,
        /// Action not allowed on banned vault.
        VaultBanned,
        /// The provided collateral was insufficient - it must be above ``MinimumCollateralVault``.
        InsufficientVaultCollateralAmount,
        /// Returned if a vault tries to register while already being registered
        VaultAlreadyRegistered,
        /// The specified vault does not exist.
        VaultNotFound,
        /// The Bitcoin Address has already been registered
        ReservedDepositAddress,
        /// Attempted to liquidate a vault that is not undercollateralized.
        VaultNotBelowLiquidationThreshold,
        /// Deposit address could not be generated with the given public key.
        InvalidPublicKey,
        /// The Max Nomination Ratio would be exceeded.
        MaxNominationRatioViolation,
        /// The collateral ceiling would be exceeded for the vault's currency
        CurrencyCeilingExceeded,

        // Errors used exclusively in RPC functions
        /// Collateralization is infinite if no tokens are issued
        NoTokensIssued,
        NoVaultWithSufficientCollateral,
        NoVaultWithSufficientTokens,
        NoVaultUnderThePremiumRedeemThreshold,

        /// Failed attempt to modify vault's collateral because it was in the wrong currency
        InvalidCurrency,

        /// Threshold was not found for the given currency
        ThresholdNotSet,
        /// Ceiling was not found for the given currency
        CeilingNotSet,

        // Unexpected errors that should never be thrown in normal operation
        ArithmeticOverflow,
        ArithmeticUnderflow,
        /// Unable to convert value
        TryIntoIntError,
    }

    /// The minimum collateral (e.g. DOT/KSM) a Vault needs to provide to register.
    #[pallet::storage]
    #[pallet::getter(fn minimum_collateral_vault)]
    pub(super) type MinimumCollateralVault<T: Config> =
        StorageMap<_, Blake2_128Concat, CurrencyId<T>, Collateral<T>, ValueQuery>;

    /// If a Vault fails to execute a correct redeem or replace, it is temporarily banned
    /// from further issue, redeem or replace requests. This value configures the duration
    /// of this ban (in number of blocks) .
    #[pallet::storage]
    #[pallet::getter(fn punishment_delay)]
    pub(super) type PunishmentDelay<T: Config> = StorageValue<_, T::BlockNumber, ValueQuery>;

    /// Determines the over-collateralization rate for collateral locked by Vaults, necessary for
    /// wrapped tokens. This threshold should be greater than the LiquidationCollateralThreshold.
    #[pallet::storage]
    pub(super) type SystemCollateralCeiling<T: Config> = StorageMap<_, Blake2_128Concat, CurrencyId<T>, Collateral<T>>;

    /// Determines the over-collateralization rate for collateral locked by Vaults, necessary for
    /// wrapped tokens. This threshold should be greater than the LiquidationCollateralThreshold.
    #[pallet::storage]
    #[pallet::getter(fn secure_collateral_threshold)]
    pub(super) type SecureCollateralThreshold<T: Config> =
        StorageMap<_, Blake2_128Concat, CurrencyId<T>, UnsignedFixedPoint<T>>;

    /// Determines the rate for the collateral rate of Vaults, at which users receive a premium,
    /// allocated from the Vault's collateral, when performing a redeem with this Vault. This
    /// threshold should be greater than the LiquidationCollateralThreshold.
    #[pallet::storage]
    #[pallet::getter(fn premium_redeem_threshold)]
    pub(super) type PremiumRedeemThreshold<T: Config> =
        StorageMap<_, Blake2_128Concat, CurrencyId<T>, UnsignedFixedPoint<T>>;
    /// Determines the lower bound for the collateral rate in issued tokens. If a Vault’s
    /// collateral rate drops below this, automatic liquidation (forced Redeem) is triggered.
    #[pallet::storage]
    #[pallet::getter(fn liquidation_collateral_threshold)]
    pub(super) type LiquidationCollateralThreshold<T: Config> =
        StorageMap<_, Blake2_128Concat, CurrencyId<T>, UnsignedFixedPoint<T>>;
    /// Account identifier of an artificial Vault maintained by the VaultRegistry to handle issued balances
    /// and collateral of liquidated Vaults. That is, when a Vault is liquidated, its balances are
    /// transferred to LiquidationVault and claims are later handled via the LiquidationVault.
    #[pallet::storage]
    #[pallet::getter(fn liquidation_vault_account_id)]
    pub(super) type LiquidationVaultAccountId<T: Config> = StorageValue<_, T::AccountId, ValueQuery>;

    #[pallet::storage]
    pub(super) type LiquidationVault<T: Config> =
        StorageMap<_, Blake2_128Concat, CurrencyId<T>, DefaultSystemVault<T>, OptionQuery>;

    /// Mapping of Vaults, using the respective Vault account identifier as key.
    #[pallet::storage]
    pub(super) type Vaults<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, DefaultVault<T>>;

    /// Mapping of reserved BTC addresses to the registered account
    #[pallet::storage]
    pub(super) type ReservedAddresses<T: Config> =
        StorageMap<_, Blake2_128Concat, BtcAddress, T::AccountId, ValueQuery>;

    /// Total collateral used for collateral tokens issued by active vaults, excluding the liquidation vault
    #[pallet::storage]
    pub(super) type TotalUserVaultCollateral<T: Config> =
        StorageMap<_, Blake2_128Concat, CurrencyId<T>, Collateral<T>, ValueQuery>;

    #[pallet::type_value]
    pub(super) fn DefaultForStorageVersion() -> Version {
        Version::V0
    }

    /// Build storage at V1 (requires default 0).
    #[pallet::storage]
    #[pallet::getter(fn storage_version)]
    pub(super) type StorageVersion<T: Config> = StorageValue<_, Version, ValueQuery, DefaultForStorageVersion>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub minimum_collateral_vault: Vec<(CurrencyId<T>, Collateral<T>)>,
        pub punishment_delay: T::BlockNumber,
        pub system_collateral_ceiling: Vec<(CurrencyId<T>, Collateral<T>)>,
        pub secure_collateral_threshold: Vec<(CurrencyId<T>, UnsignedFixedPoint<T>)>,
        pub premium_redeem_threshold: Vec<(CurrencyId<T>, UnsignedFixedPoint<T>)>,
        pub liquidation_collateral_threshold: Vec<(CurrencyId<T>, UnsignedFixedPoint<T>)>,
    }

    #[cfg(feature = "std")]
    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                minimum_collateral_vault: Default::default(),
                punishment_delay: Default::default(),
                system_collateral_ceiling: Default::default(),
                secure_collateral_threshold: Default::default(),
                premium_redeem_threshold: Default::default(),
                liquidation_collateral_threshold: Default::default(),
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
        fn build(&self) {
            PunishmentDelay::<T>::put(self.punishment_delay);
            for (currency_id, minimum) in self.minimum_collateral_vault.iter() {
                MinimumCollateralVault::<T>::insert(currency_id, minimum);
            }
            for (currency_id, ceiling) in self.system_collateral_ceiling.iter() {
                SystemCollateralCeiling::<T>::insert(currency_id, ceiling);
            }
            for (currency_id, threshold) in self.secure_collateral_threshold.iter() {
                SecureCollateralThreshold::<T>::insert(currency_id, threshold);
            }
            for (currency_id, threshold) in self.premium_redeem_threshold.iter() {
                PremiumRedeemThreshold::<T>::insert(currency_id, threshold);
            }
            for (currency_id, threshold) in self.liquidation_collateral_threshold.iter() {
                LiquidationCollateralThreshold::<T>::insert(currency_id, threshold);
            }
            StorageVersion::<T>::put(Version::V1);
            LiquidationVaultAccountId::<T>::put::<T::AccountId>(<T as Config>::PalletId::get().into_account());
        }
    }
}

// "Internal" functions, callable by code.
#[cfg_attr(test, mockable)]
impl<T: Config> Pallet<T> {
    fn _offchain_worker() {
        for vault in Self::undercollateralized_vaults() {
            log::info!("Reporting vault {:?}", vault);
            let call = Call::report_undercollateralized_vault(vault);
            let _ = SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(call.into());
        }
    }

    /// Public functions

    pub fn _register_vault(
        vault_id: &T::AccountId,
        collateral: Collateral<T>,
        public_key: BtcPublicKey,
        currency_id: CurrencyId<T>,
    ) -> DispatchResult {
        let amount = Amount::new(collateral, currency_id);

        ensure!(
            amount.ge(&Self::get_minimum_collateral_vault(currency_id))?,
            Error::<T>::InsufficientVaultCollateralAmount
        );
        ensure!(!Self::vault_exists(vault_id), Error::<T>::VaultAlreadyRegistered);

        let vault = Vault::new(vault_id.clone(), public_key, currency_id);
        Self::insert_vault(vault_id, vault);

        Self::try_deposit_collateral(vault_id, &amount)?;

        Self::deposit_event(Event::<T>::RegisterVault(vault_id.clone(), collateral, currency_id));

        Ok(())
    }

    pub fn get_vault_from_id(vault_id: &T::AccountId) -> Result<DefaultVault<T>, DispatchError> {
        Vaults::<T>::get(vault_id).ok_or(Error::<T>::VaultNotFound.into())
    }

    pub fn get_backing_collateral(vault_id: &T::AccountId) -> Result<Amount<T>, DispatchError> {
        let stake = ext::staking::total_current_stake::<T>(vault_id)?
            .try_into()
            .map_err(|_| Error::<T>::TryIntoIntError)?;
        Ok(Amount::new(stake, Self::get_collateral_currency(vault_id)?))
    }

    pub fn get_liquidated_collateral(vault_id: &T::AccountId) -> Result<Amount<T>, DispatchError> {
        let vault = Self::get_vault_from_id(vault_id)?;
        Ok(Amount::new(vault.liquidated_collateral, vault.currency_id))
    }

    /// Like get_vault_from_id, but additionally checks that the vault is active
    pub fn get_active_vault_from_id(vault_id: &T::AccountId) -> Result<DefaultVault<T>, DispatchError> {
        let vault = Self::get_vault_from_id(vault_id)?;
        ensure!(
            matches!(vault.status, VaultStatus::Active(_)),
            Error::<T>::VaultNotFound
        );
        Ok(vault)
    }

    /// Deposit an `amount` of collateral to be used for collateral tokens
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault
    /// * `amount` - the amount of collateral
    pub fn try_deposit_collateral(vault_id: &T::AccountId, amount: &Amount<T>) -> DispatchResult {
        // ensure the vault is active
        let _vault = Self::get_active_rich_vault_from_id(vault_id)?;

        // will fail if collateral ceiling exceeded
        Self::try_increase_total_backing_collateral(amount)?;
        // will fail if free_balance is insufficient
        amount.lock_on(vault_id)?;

        // Deposit `amount` of stake in the pool
        ext::staking::deposit_stake::<T>(T::GetWrappedCurrencyId::get(), vault_id, vault_id, amount)?;

        Ok(())
    }

    /// Withdraw an `amount` of collateral without checking collateralization
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault
    /// * `amount` - the amount of collateral
    pub fn force_withdraw_collateral(vault_id: &T::AccountId, amount: &Amount<T>) -> DispatchResult {
        let _currency_id = Self::get_collateral_currency(vault_id)?;

        // will fail if reserved_balance is insufficient
        amount.unlock_on(vault_id)?;
        Self::decrease_total_backing_collateral(amount)?;

        // Withdraw `amount` of stake from the pool
        ext::staking::withdraw_stake::<T>(T::GetWrappedCurrencyId::get(), vault_id, vault_id, amount)?;

        Ok(())
    }

    /// Withdraw an `amount` of collateral, ensuring that the vault is sufficiently
    /// over-collateralized
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault
    /// * `amount` - the amount of collateral
    pub fn try_withdraw_collateral(vault_id: &T::AccountId, amount: &Amount<T>) -> DispatchResult {
        ensure!(
            Self::is_allowed_to_withdraw_collateral(vault_id, amount)?,
            Error::<T>::InsufficientCollateral
        );
        ensure!(
            Self::is_max_nomination_ratio_preserved(vault_id, amount)?,
            Error::<T>::MaxNominationRatioViolation
        );
        Self::force_withdraw_collateral(vault_id, amount)
    }

    pub fn is_max_nomination_ratio_preserved(
        vault_id: &T::AccountId,
        amount: &Amount<T>,
    ) -> Result<bool, DispatchError> {
        let vault_collateral = Self::compute_collateral(vault_id)?;
        let backing_collateral = Self::get_backing_collateral(vault_id)?;
        let current_nomination = backing_collateral.checked_sub(&vault_collateral)?;
        let new_vault_collateral = vault_collateral.checked_sub(&amount)?;
        let _currency_id = Self::get_collateral_currency(&vault_id)?;
        let max_nomination_after_withdrawal = Self::get_max_nominatable_collateral(&new_vault_collateral)?;
        Ok(current_nomination.le(&max_nomination_after_withdrawal)?)
    }

    /// Checks if the vault would be above the secure threshold after withdrawing collateral
    pub fn is_allowed_to_withdraw_collateral(
        vault_id: &T::AccountId,
        amount: &Amount<T>,
    ) -> Result<bool, DispatchError> {
        let vault = Self::get_rich_vault_from_id(vault_id)?;

        let new_collateral = match Self::get_backing_collateral(vault_id)?.checked_sub(&amount) {
            Ok(x) => x,
            Err(x) if x == currency::Error::<T>::ArithmeticUnderflow.into() => return Ok(false),
            Err(x) => return Err(x),
        };

        let is_below_threshold =
            Pallet::<T>::is_collateral_below_secure_threshold(&new_collateral, &vault.backed_tokens()?)?;
        Ok(!is_below_threshold)
    }

    pub fn transfer_funds_saturated(
        from: CurrencySource<T>,
        to: CurrencySource<T>,
        amount: &Amount<T>,
    ) -> Result<Amount<T>, DispatchError> {
        let available_amount = from.current_balance(amount.currency())?;
        let amount = if available_amount.lt(&amount)? {
            available_amount
        } else {
            amount.clone()
        };
        Self::transfer_funds(from, to, &amount)?;
        Ok(amount)
    }

    fn slash_backing_collateral(vault_id: &T::AccountId, amount: &Amount<T>) -> DispatchResult {
        amount.unlock_on(vault_id)?;
        Self::decrease_total_backing_collateral(amount)?;
        ext::staking::slash_stake::<T>(T::GetWrappedCurrencyId::get(), vault_id, amount)?;
        Ok(())
    }

    pub fn transfer_funds(from: CurrencySource<T>, to: CurrencySource<T>, amount: &Amount<T>) -> DispatchResult {
        match from {
            CurrencySource::Collateral(ref account) => {
                ensure!(
                    Self::get_collateral_currency(account)? == amount.currency(),
                    Error::<T>::InvalidCurrency
                );
                Self::slash_backing_collateral(account, amount)?;
            }
            CurrencySource::Griefing(_) => {
                amount.unlock_on(&from.account_id())?;
            }
            CurrencySource::LiquidatedCollateral(_) | CurrencySource::LiquidationVault => {
                Self::decrease_total_backing_collateral(amount)?;
                amount.unlock_on(&from.account_id())?;
            }
            CurrencySource::FreeBalance(_) => {
                // do nothing
            }
        };

        // move from sender's free balance to receiver's free balance
        amount.transfer(&from.account_id(), &to.account_id())?;

        // move receiver funds from free balance to specified currency source
        match to {
            CurrencySource::Collateral(ref account) => {
                // todo: do we need to do this for griefing as well?
                ensure!(
                    Self::get_collateral_currency(account)? == amount.currency(),
                    Error::<T>::InvalidCurrency
                );
                Self::try_deposit_collateral(account, amount)?;
            }
            CurrencySource::Griefing(_) => {
                amount.lock_on(&to.account_id())?;
            }
            CurrencySource::LiquidationVault | CurrencySource::LiquidatedCollateral(_) => {
                Self::try_increase_total_backing_collateral(amount)?;
                amount.lock_on(&to.account_id())?;
            }
            CurrencySource::FreeBalance(_) => {
                // do nothing
            }
        };

        Ok(())
    }

    pub fn get_collateral_currency(vault_id: &T::AccountId) -> Result<CurrencyId<T>, DispatchError> {
        let currency_id = Self::get_vault_from_id(vault_id)?.currency_id;
        Ok(currency_id)
    }

    /// Checks if the vault has sufficient collateral to increase the to-be-issued tokens, and
    /// if so, increases it
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault from which to increase to-be-issued tokens
    /// * `tokens` - the amount of tokens to be reserved
    pub fn try_increase_to_be_issued_tokens(vault_id: &T::AccountId, tokens: &Amount<T>) -> Result<(), DispatchError> {
        let mut vault = Self::get_active_rich_vault_from_id(&vault_id)?;

        let issuable_tokens = vault.issuable_tokens()?;
        ensure!(issuable_tokens.ge(&tokens)?, Error::<T>::ExceedingVaultLimit);
        vault.increase_to_be_issued(tokens)?;

        Self::deposit_event(Event::<T>::IncreaseToBeIssuedTokens(vault.id(), tokens.amount()));
        Ok(())
    }

    /// Registers a btc address
    ///
    /// # Arguments
    /// * `issue_id` - secure id for generating deposit address
    pub fn register_deposit_address(vault_id: &T::AccountId, issue_id: H256) -> Result<BtcAddress, DispatchError> {
        let mut vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        let btc_address = vault.new_deposit_address(issue_id)?;
        Self::deposit_event(Event::<T>::RegisterAddress(vault.id(), btc_address));
        Ok(btc_address)
    }

    /// returns the amount of tokens that a vault can request to be replaced on top of the
    /// current to-be-replaced tokens
    pub fn requestable_to_be_replaced_tokens(vault_id: &T::AccountId) -> Result<Amount<T>, DispatchError> {
        let vault = Self::get_active_rich_vault_from_id(&vault_id)?;

        vault
            .issued_tokens()
            .checked_sub(&vault.to_be_replaced_tokens())?
            .checked_sub(&vault.to_be_redeemed_tokens())
    }

    /// returns the new total to-be-replaced and replace-collateral
    pub fn try_increase_to_be_replaced_tokens(
        vault_id: &T::AccountId,
        tokens: &Amount<T>,
        griefing_collateral: &Amount<T>,
    ) -> Result<(Amount<T>, Amount<T>), DispatchError> {
        let mut vault = Self::get_active_rich_vault_from_id(&vault_id)?;

        let new_to_be_replaced = vault.to_be_replaced_tokens().checked_add(&tokens)?;
        let new_collateral = vault.replace_collateral().checked_add(&griefing_collateral)?;
        let total_decreasing_tokens = new_to_be_replaced.checked_add(&vault.to_be_redeemed_tokens())?;

        ensure!(
            total_decreasing_tokens.le(&vault.issued_tokens())?,
            Error::<T>::InsufficientTokensCommitted
        );

        vault.set_to_be_replaced_amount(&new_to_be_replaced, &new_collateral)?;

        Self::deposit_event(Event::<T>::IncreaseToBeReplacedTokens(vault.id(), tokens.amount()));

        Ok((new_to_be_replaced, new_collateral))
    }

    pub fn decrease_to_be_replaced_tokens(
        vault_id: &T::AccountId,
        tokens: &Amount<T>,
    ) -> Result<(Amount<T>, Amount<T>), DispatchError> {
        let mut vault = Self::get_rich_vault_from_id(&vault_id)?;

        let initial_to_be_replaced = Amount::new(vault.data.to_be_replaced_tokens, T::GetWrappedCurrencyId::get());
        let initial_griefing_collateral =
            Amount::new(vault.data.replace_collateral, T::GetGriefingCollateralCurrencyId::get());

        let used_tokens = tokens.min(&initial_to_be_replaced)?;

        let used_collateral =
            Self::calculate_collateral(&initial_griefing_collateral, &used_tokens, &initial_to_be_replaced)?;

        // make sure we don't use too much if a rounding error occurs
        let used_collateral = used_collateral.min(&initial_griefing_collateral)?;

        let new_to_be_replaced = initial_to_be_replaced.checked_sub(&used_tokens)?;
        let new_collateral = initial_griefing_collateral.checked_sub(&used_collateral)?;

        vault.set_to_be_replaced_amount(&new_to_be_replaced, &new_collateral)?;

        Self::deposit_event(Event::<T>::DecreaseToBeReplacedTokens(vault.id(), tokens.amount()));

        Ok((used_tokens, used_collateral))
    }

    /// Decreases the amount of tokens to be issued in the next issue request from the
    /// vault, or from the liquidation vault if the vault is liquidated
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault from which to decrease to-be-issued tokens
    /// * `tokens` - the amount of tokens to be unreserved
    pub fn decrease_to_be_issued_tokens(vault_id: &T::AccountId, tokens: &Amount<T>) -> DispatchResult {
        let mut vault = Self::get_rich_vault_from_id(vault_id)?;
        vault.decrease_to_be_issued(tokens)?;

        Self::deposit_event(Event::<T>::DecreaseToBeIssuedTokens(vault_id.clone(), tokens.amount()));
        Ok(())
    }

    /// Issues an amount of `tokens` tokens for the given `vault_id`
    /// At this point, the to-be-issued tokens assigned to a vault are decreased
    /// and the issued tokens balance is increased by the amount of issued tokens.
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault from which to issue tokens
    /// * `tokens` - the amount of tokens to issue
    ///
    /// # Errors
    /// * `VaultNotFound` - if no vault exists for the given `vault_id`
    /// * `InsufficientTokensCommitted` - if the amount of tokens reserved is too low
    pub fn issue_tokens(vault_id: &T::AccountId, tokens: &Amount<T>) -> DispatchResult {
        let mut vault = Self::get_rich_vault_from_id(&vault_id)?;
        vault.issue_tokens(tokens)?;
        Self::deposit_event(Event::<T>::IssueTokens(vault.id(), tokens.amount()));
        Ok(())
    }

    /// Adds an amount tokens to the to-be-redeemed tokens balance of a vault.
    /// This function serves as a prevention against race conditions in the
    /// redeem and replace procedures. If, for example, a vault would receive
    /// two redeem requests at the same time that have a higher amount of tokens
    ///  to be issued than his issuedTokens balance, one of the two redeem
    /// requests should be rejected.
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault from which to increase to-be-redeemed tokens
    /// * `tokens` - the amount of tokens to be redeemed
    ///
    /// # Errors
    /// * `VaultNotFound` - if no vault exists for the given `vault_id`
    /// * `InsufficientTokensCommitted` - if the amount of redeemable tokens is too low
    pub fn try_increase_to_be_redeemed_tokens(vault_id: &T::AccountId, tokens: &Amount<T>) -> DispatchResult {
        let mut vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        let redeemable = vault.issued_tokens().checked_sub(&vault.to_be_redeemed_tokens())?;
        ensure!(redeemable.ge(&tokens)?, Error::<T>::InsufficientTokensCommitted);

        vault.increase_to_be_redeemed(tokens)?;

        Self::deposit_event(Event::<T>::IncreaseToBeRedeemedTokens(vault.id(), tokens.amount()));
        Ok(())
    }

    /// Subtracts an amount tokens from the to-be-redeemed tokens balance of a vault.
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault from which to decrease to-be-redeemed tokens
    /// * `tokens` - the amount of tokens to be redeemed
    ///
    /// # Errors
    /// * `VaultNotFound` - if no vault exists for the given `vault_id`
    /// * `InsufficientTokensCommitted` - if the amount of to-be-redeemed tokens is too low
    pub fn decrease_to_be_redeemed_tokens(vault_id: &T::AccountId, tokens: &Amount<T>) -> DispatchResult {
        let mut vault = Self::get_rich_vault_from_id(&vault_id)?;
        vault.decrease_to_be_redeemed(tokens)?;

        Self::deposit_event(Event::<T>::DecreaseToBeRedeemedTokens(vault.id(), tokens.amount()));
        Ok(())
    }

    /// Decreases the amount of tokens f a redeem request is not fulfilled
    /// Removes the amount of tokens assigned to the to-be-redeemed tokens.
    /// At this point, we consider the tokens lost and the issued tokens are
    /// removed from the vault
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault from which to decrease tokens
    /// * `tokens` - the amount of tokens to be decreased
    /// * `user_id` - the id of the user making the redeem request
    pub fn decrease_tokens(vault_id: &T::AccountId, user_id: &T::AccountId, tokens: &Amount<T>) -> DispatchResult {
        // decrease to-be-redeemed and issued
        let mut vault = Self::get_rich_vault_from_id(&vault_id)?;
        vault.decrease_tokens(tokens)?;

        Self::deposit_event(Event::<T>::DecreaseTokens(vault.id(), user_id.clone(), tokens.amount()));
        Ok(())
    }

    /// Decreases the amount of collateral held after liquidation for any remaining to_be_redeemed tokens.
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault
    /// * `amount` - the amount of collateral to decrement
    pub fn decrease_liquidated_collateral(vault_id: &T::AccountId, amount: &Amount<T>) -> DispatchResult {
        let mut vault = Self::get_rich_vault_from_id(vault_id)?;
        vault.decrease_liquidated_collateral(amount)?;
        Ok(())
    }

    /// Reduces the to-be-redeemed tokens when a redeem request completes
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault from which to redeem tokens
    /// * `tokens` - the amount of tokens to be decreased
    /// * `premium` - amount of collateral to be rewarded to the redeemer if the vault is not liquidated yet
    /// * `redeemer_id` - the id of the redeemer
    pub fn redeem_tokens(
        vault_id: &T::AccountId,
        tokens: &Amount<T>,
        premium: &Amount<T>,
        redeemer_id: &T::AccountId,
    ) -> DispatchResult {
        let mut vault = Self::get_rich_vault_from_id(&vault_id)?;

        // need to read before we decrease it
        let to_be_redeemed_tokens = vault.to_be_redeemed_tokens();

        vault.decrease_to_be_redeemed(tokens)?;
        vault.decrease_issued(tokens)?;

        if !vault.data.is_liquidated() {
            if premium.is_zero() {
                Self::deposit_event(Event::<T>::RedeemTokens(vault.id(), tokens.amount()));
            } else {
                Self::transfer_funds(
                    CurrencySource::Collateral(vault_id.clone()),
                    CurrencySource::FreeBalance(redeemer_id.clone()),
                    premium,
                )?;

                Self::deposit_event(Event::<T>::RedeemTokensPremium(
                    vault_id.clone(),
                    tokens.amount(),
                    premium.amount(),
                    redeemer_id.clone(),
                ));
            }
        } else {
            // NOTE: previously we calculated the amount to release based on the Vault's `backing_collateral`
            // but this may now be wrong in the pull-based approach if the Vault is left with excess collateral
            let to_be_released =
                Self::calculate_collateral(&vault.liquidated_collateral(), tokens, &to_be_redeemed_tokens)?;
            Self::decrease_total_backing_collateral(&to_be_released)?;
            vault.decrease_liquidated_collateral(&to_be_released)?;

            // release the collateral back to the free balance of the vault
            to_be_released.unlock_on(vault_id)?;

            Self::deposit_event(Event::<T>::RedeemTokensLiquidatedVault(
                vault_id.clone(),
                tokens.amount(),
                to_be_released.amount(),
            ));
        }

        Ok(())
    }

    /// Handles redeem requests which are executed against the LiquidationVault.
    /// Reduces the issued token of the LiquidationVault and slashes the
    /// corresponding amount of collateral.
    ///
    /// # Arguments
    /// * `currency_id` - the currency being redeemed
    /// * `redeemer_id` - the account of the user redeeming issued tokens
    /// * `tokens` - the amount of tokens to be redeemed in collateral with the LiquidationVault, denominated in BTC
    ///
    /// # Errors
    /// * `InsufficientTokensCommitted` - if the amount of tokens issued by the liquidation vault is too low
    /// * `InsufficientFunds` - if the liquidation vault does not have enough collateral to transfer
    pub fn redeem_tokens_liquidation(
        currency_id: CurrencyId<T>,
        redeemer_id: &T::AccountId,
        amount_btc: &Amount<T>, // todo: maybe change interface
    ) -> DispatchResult {
        let mut liquidation_vault = Self::get_rich_liquidation_vault(currency_id);

        ensure!(
            liquidation_vault.redeemable_tokens()?.ge(&amount_btc)?,
            Error::<T>::InsufficientTokensCommitted
        );

        // transfer liquidated collateral to redeemer
        let to_transfer = Self::calculate_collateral(
            &CurrencySource::<T>::LiquidationVault.current_balance(currency_id)?,
            amount_btc,
            &liquidation_vault.backed_tokens()?,
        )?;

        Self::transfer_funds(
            CurrencySource::LiquidationVault,
            CurrencySource::FreeBalance(redeemer_id.clone()),
            &to_transfer,
        )?;

        liquidation_vault.decrease_issued(amount_btc)?;

        Self::deposit_event(Event::<T>::RedeemTokensLiquidation(
            redeemer_id.clone(),
            amount_btc.amount(),
            to_transfer.amount(),
        ));

        Ok(())
    }

    /// Replaces the old vault by the new vault by transferring tokens
    /// from the old vault to the new one
    ///
    /// # Arguments
    /// * `old_vault_id` - the id of the old vault
    /// * `new_vault_id` - the id of the new vault
    /// * `tokens` - the amount of tokens to be transferred from the old to the new vault
    /// * `collateral` - the collateral to be locked by the new vault
    ///
    /// # Errors
    /// * `VaultNotFound` - if either the old or new vault does not exist
    /// * `InsufficientTokensCommitted` - if the amount of tokens of the old vault is too low
    /// * `InsufficientFunds` - if the new vault does not have enough collateral to lock
    pub fn replace_tokens(
        old_vault_id: &T::AccountId,
        new_vault_id: &T::AccountId,
        tokens: &Amount<T>,
        collateral: &Amount<T>,
    ) -> DispatchResult {
        let mut old_vault = Self::get_rich_vault_from_id(&old_vault_id)?;
        let mut new_vault = Self::get_rich_vault_from_id(&new_vault_id)?;

        if old_vault.data.is_liquidated() {
            let to_be_released = Self::calculate_collateral(
                &old_vault.liquidated_collateral(),
                tokens,
                &old_vault.to_be_redeemed_tokens(),
            )?;
            old_vault.decrease_liquidated_collateral(&to_be_released)?;

            // deposit old-vault's collateral (this was withdrawn on liquidation)
            ext::staking::deposit_stake::<T>(
                T::GetWrappedCurrencyId::get(),
                old_vault_id,
                old_vault_id,
                &to_be_released,
            )?;
        }

        old_vault.decrease_tokens(tokens)?;
        new_vault.issue_tokens(tokens)?;

        Self::deposit_event(Event::<T>::ReplaceTokens(
            old_vault_id.clone(),
            new_vault_id.clone(),
            tokens.amount(),
            collateral.amount(),
        ));
        Ok(())
    }

    /// Cancels a replace - which in the normal case decreases the old-vault's
    /// to-be-redeemed tokens, and the new-vault's to-be-issued tokens.
    /// When one or both of the vaults have been liquidated, this function also
    /// updates the liquidation vault.
    ///
    /// # Arguments
    /// * `old_vault_id` - the id of the old vault
    /// * `new_vault_id` - the id of the new vault
    /// * `tokens` - the amount of tokens to be transferred from the old to the new vault
    pub fn cancel_replace_tokens(
        old_vault_id: &T::AccountId,
        new_vault_id: &T::AccountId,
        tokens: &Amount<T>,
    ) -> DispatchResult {
        let mut old_vault = Self::get_rich_vault_from_id(&old_vault_id)?;
        let mut new_vault = Self::get_rich_vault_from_id(&new_vault_id)?;

        if old_vault.data.is_liquidated() {
            let to_be_transferred = Self::calculate_collateral(
                &old_vault.liquidated_collateral(),
                tokens,
                &old_vault.to_be_redeemed_tokens(),
            )?;
            old_vault.decrease_liquidated_collateral(&to_be_transferred)?;

            // transfer old-vault's collateral to liquidation_vault
            Self::transfer_funds(
                CurrencySource::LiquidatedCollateral(old_vault_id.clone()),
                CurrencySource::LiquidationVault,
                &to_be_transferred,
            )?;
        }

        old_vault.decrease_to_be_redeemed(tokens)?;
        new_vault.decrease_to_be_issued(tokens)?;

        Ok(())
    }

    fn undercollateralized_vaults() -> impl Iterator<Item = T::AccountId> {
        <Vaults<T>>::iter().filter_map(|(vault_id, vault)| {
            if let Some(liquidation_threshold) = Self::liquidation_collateral_threshold(vault.currency_id) {
                if Self::is_vault_below_liquidation_threshold(&vault, liquidation_threshold).unwrap_or(false) {
                    return Some(vault_id);
                }
            }
            None
        })
    }

    /// Liquidates a vault, transferring all of its token balances to the `LiquidationVault`.
    /// Delegates to `liquidate_vault_with_status`, using `Liquidated` status
    pub fn liquidate_vault(vault_id: &T::AccountId) -> Result<Amount<T>, DispatchError> {
        Self::liquidate_vault_with_status(vault_id, VaultStatus::Liquidated, None)
    }

    /// Liquidates a vault, transferring all of its token balances to the
    /// `LiquidationVault`, as well as the collateral.
    ///
    /// # Arguments
    /// * `vault_id` - the id of the vault to liquidate
    /// * `status` - status with which to liquidate the vault
    pub fn liquidate_vault_with_status(
        vault_id: &T::AccountId,
        status: VaultStatus,
        reporter: Option<T::AccountId>,
    ) -> Result<Amount<T>, DispatchError> {
        let mut vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        let backing_collateral = vault.get_total_collateral()?;
        let vault_orig = vault.data.clone();

        let to_slash = vault.liquidate(status, reporter)?;

        Self::deposit_event(Event::<T>::LiquidateVault(
            vault_id.clone(),
            vault_orig.issued_tokens,
            vault_orig.to_be_issued_tokens,
            vault_orig.to_be_redeemed_tokens,
            vault_orig.to_be_replaced_tokens,
            backing_collateral.amount(),
            status,
            vault_orig.replace_collateral,
        ));
        Ok(to_slash)
    }

    pub fn try_increase_total_backing_collateral(amount: &Amount<T>) -> DispatchResult {
        let new = Self::get_total_user_vault_collateral(amount.currency())?.checked_add(&amount)?;

        let limit = Self::get_collateral_ceiling(amount.currency())?;
        ensure!(new.le(&limit)?, Error::<T>::CurrencyCeilingExceeded);

        TotalUserVaultCollateral::<T>::insert(&amount.currency(), new.amount());

        Ok(())
    }

    pub fn decrease_total_backing_collateral(amount: &Amount<T>) -> DispatchResult {
        let new = Self::get_total_user_vault_collateral(amount.currency())?.checked_sub(amount)?;

        TotalUserVaultCollateral::<T>::insert(&amount.currency(), new.amount());

        Ok(())
    }

    pub fn insert_vault(id: &T::AccountId, vault: DefaultVault<T>) {
        Vaults::<T>::insert(id, vault)
    }

    pub fn ban_vault(vault_id: T::AccountId) -> DispatchResult {
        let height = ext::security::active_block_number::<T>();
        let mut vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        let banned_until = height + Self::punishment_delay();
        vault.ban_until(banned_until);
        Self::deposit_event(Event::<T>::BanVault(vault.id(), banned_until));
        Ok(())
    }

    pub fn _ensure_not_banned(vault_id: &T::AccountId) -> DispatchResult {
        let vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        vault.ensure_not_banned()
    }

    /// Threshold checks
    pub fn is_vault_below_secure_threshold(vault_id: &T::AccountId) -> Result<bool, DispatchError> {
        let currency_id = Self::get_collateral_currency(vault_id)?;
        let threshold = Self::secure_collateral_threshold(currency_id).ok_or(Error::<T>::ThresholdNotSet)?;
        Self::is_vault_below_threshold(&vault_id, threshold)
    }

    pub fn is_vault_liquidated(vault_id: &T::AccountId) -> Result<bool, DispatchError> {
        Ok(Self::get_vault_from_id(&vault_id)?.is_liquidated())
    }

    pub fn is_vault_below_premium_threshold(vault_id: &T::AccountId) -> Result<bool, DispatchError> {
        let currency_id = Self::get_collateral_currency(vault_id)?;
        let threshold = Self::premium_redeem_threshold(currency_id).ok_or(Error::<T>::ThresholdNotSet)?;
        Self::is_vault_below_threshold(&vault_id, threshold)
    }

    /// check if the vault is below the liquidation threshold.
    pub fn is_vault_below_liquidation_threshold(
        vault: &DefaultVault<T>,
        liquidation_threshold: UnsignedFixedPoint<T>,
    ) -> Result<bool, DispatchError> {
        Self::is_collateral_below_threshold(
            &Self::get_backing_collateral(&vault.id)?,
            &Amount::new(vault.issued_tokens, T::GetWrappedCurrencyId::get()),
            liquidation_threshold,
        )
    }

    pub fn is_collateral_below_secure_threshold(
        collateral: &Amount<T>,
        btc_amount: &Amount<T>,
    ) -> Result<bool, DispatchError> {
        let threshold = Self::secure_collateral_threshold(collateral.currency()).ok_or(Error::<T>::ThresholdNotSet)?;
        Self::is_collateral_below_threshold(collateral, btc_amount, threshold)
    }

    pub fn set_collateral_ceiling(currency_id: CurrencyId<T>, ceiling: Collateral<T>) {
        SystemCollateralCeiling::<T>::insert(currency_id, ceiling);
    }

    pub fn set_secure_collateral_threshold(currency_id: CurrencyId<T>, threshold: UnsignedFixedPoint<T>) {
        SecureCollateralThreshold::<T>::insert(currency_id, threshold);
    }

    pub fn set_premium_redeem_threshold(currency_id: CurrencyId<T>, threshold: UnsignedFixedPoint<T>) {
        PremiumRedeemThreshold::<T>::insert(currency_id, threshold);
    }

    pub fn set_liquidation_collateral_threshold(currency_id: CurrencyId<T>, threshold: UnsignedFixedPoint<T>) {
        LiquidationCollateralThreshold::<T>::insert(currency_id, threshold);
    }

    /// return (collateral * Numerator) / denominator, used when dealing with liquidated vaults
    pub fn calculate_collateral(
        collateral: &Amount<T>,
        numerator: &Amount<T>,
        denominator: &Amount<T>,
    ) -> Result<Amount<T>, DispatchError> {
        if numerator.is_zero() && denominator.is_zero() {
            return Ok(collateral.clone());
        }

        let currency = collateral.currency();

        let collateral: U256 = collateral.amount().into();
        let numerator: U256 = numerator.amount().into();
        let denominator: U256 = denominator.amount().into();

        let amount = collateral
            .checked_mul(numerator)
            .ok_or(Error::<T>::ArithmeticOverflow)?
            .checked_div(denominator)
            .ok_or(Error::<T>::ArithmeticUnderflow)?
            .try_into()
            .map_err(|_| Error::<T>::TryIntoIntError)?;
        Ok(Amount::new(amount, currency))
    }

    /// RPC

    /// Get all vaults that:
    /// - are below the premium redeem threshold, and
    /// - have a non-zero amount of redeemable tokens, and thus
    /// - are not banned
    ///
    /// Maybe returns a tuple of (VaultId, RedeemableTokens)
    /// The redeemable tokens are the currently vault.issued_tokens - the vault.to_be_redeemed_tokens
    pub fn get_premium_redeem_vaults() -> Result<Vec<(T::AccountId, Amount<T>)>, DispatchError> {
        let mut suitable_vaults = Vaults::<T>::iter()
            .filter_map(|(account_id, vault)| {
                let rich_vault: RichVault<T> = vault.into();

                let redeemable_tokens = rich_vault.redeemable_tokens().ok()?;

                if !redeemable_tokens.is_zero() && Self::is_vault_below_premium_threshold(&account_id).unwrap_or(false)
                {
                    Some((account_id, redeemable_tokens))
                } else {
                    None
                }
            })
            .collect::<Vec<(_, _)>>();

        if suitable_vaults.is_empty() {
            Err(Error::<T>::NoVaultUnderThePremiumRedeemThreshold.into())
        } else {
            suitable_vaults.sort_by(|a, b| b.1.amount().cmp(&a.1.amount()));
            Ok(suitable_vaults)
        }
    }

    /// Get all vaults with non-zero issuable tokens, ordered in descending order of this amount
    pub fn get_vaults_with_issuable_tokens() -> Result<Vec<(T::AccountId, Amount<T>)>, DispatchError> {
        let mut vaults_with_issuable_tokens = Vaults::<T>::iter()
            .filter_map(|(account_id, _vault)| {
                // NOTE: we are not checking if the vault accepts new issues here - if not, then
                // get_issuable_tokens_from_vault will return 0, and we will filter them out below

                // iterator returns tuple of (AccountId, Vault<T>),
                match Self::get_issuable_tokens_from_vault(account_id.clone()).ok() {
                    Some(issuable_tokens) => {
                        if !issuable_tokens.is_zero() {
                            Some((account_id, issuable_tokens))
                        } else {
                            None
                        }
                    }
                    None => None,
                }
            })
            .collect::<Vec<(_, _)>>();

        vaults_with_issuable_tokens.sort_by(|a, b| b.1.amount().cmp(&a.1.amount()));
        Ok(vaults_with_issuable_tokens)
    }

    /// Get all vaults with non-zero issued (thus redeemable) tokens, ordered in descending order of this amount
    pub fn get_vaults_with_redeemable_tokens() -> Result<Vec<(T::AccountId, Amount<T>)>, DispatchError> {
        // find all vault accounts with sufficient collateral
        let mut vaults_with_redeemable_tokens = Vaults::<T>::iter()
            .filter_map(|(account_id, vault)| {
                let vault = Into::<RichVault<T>>::into(vault);
                let redeemable_tokens = vault.redeemable_tokens().ok()?;
                if !redeemable_tokens.is_zero() {
                    Some((account_id, redeemable_tokens))
                } else {
                    None
                }
            })
            .collect::<Vec<(_, _)>>();

        vaults_with_redeemable_tokens.sort_by(|a, b| b.1.amount().cmp(&a.1.amount()));
        Ok(vaults_with_redeemable_tokens)
    }

    /// Get the amount of tokens a vault can issue
    pub fn get_issuable_tokens_from_vault(vault_id: T::AccountId) -> Result<Amount<T>, DispatchError> {
        let vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        // make sure the vault accepts new issue requests.
        // NOTE: get_vaults_with_issuable_tokens depends on this check
        if vault.data.status != VaultStatus::Active(true) {
            Ok(Amount::new(0u32.into(), vault.data.currency_id))
        } else {
            vault.issuable_tokens()
        }
    }

    /// Get the amount of tokens issued by a vault
    pub fn get_to_be_issued_tokens_from_vault(vault_id: T::AccountId) -> Result<Amount<T>, DispatchError> {
        let vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        Ok(vault.to_be_issued_tokens())
    }

    /// Get the current collateralization of a vault
    pub fn get_collateralization_from_vault(
        vault_id: T::AccountId,
        only_issued: bool,
    ) -> Result<UnsignedFixedPoint<T>, DispatchError> {
        let vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        let collateral = vault.get_total_collateral()?;
        Self::get_collateralization_from_vault_and_collateral(vault_id, &collateral, only_issued)
    }

    pub fn get_collateralization_from_vault_and_collateral(
        vault_id: T::AccountId,
        collateral: &Amount<T>,
        only_issued: bool,
    ) -> Result<UnsignedFixedPoint<T>, DispatchError> {
        let vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        let issued_tokens = if only_issued {
            vault.issued_tokens()
        } else {
            vault.backed_tokens()?
        };

        ensure!(!issued_tokens.is_zero(), Error::<T>::NoTokensIssued);

        // convert the collateral to wrapped
        let collateral_in_wrapped = collateral.convert_to(T::GetWrappedCurrencyId::get())?;

        Self::get_collateralization(&collateral_in_wrapped, &issued_tokens)
    }

    /// Gets the minimum amount of collateral required for the given amount of btc
    /// with the current threshold and exchange rate
    ///
    /// # Arguments
    /// * `amount_btc` - the amount of wrapped
    pub fn get_required_collateral_for_wrapped(
        amount_btc: &Amount<T>,
        currency_id: CurrencyId<T>,
    ) -> Result<Amount<T>, DispatchError> {
        let threshold = Self::secure_collateral_threshold(currency_id).ok_or(Error::<T>::ThresholdNotSet)?;
        let collateral = Self::get_required_collateral_for_wrapped_with_threshold(amount_btc, threshold, currency_id)?;
        Ok(collateral)
    }

    /// Get the amount of collateral required for the given vault to be at the
    /// current SecureCollateralThreshold with the current exchange rate
    pub fn get_required_collateral_for_vault(vault_id: T::AccountId) -> Result<Amount<T>, DispatchError> {
        let vault = Self::get_active_rich_vault_from_id(&vault_id)?;
        let issued_tokens = vault.backed_tokens()?;

        let required_collateral = Self::get_required_collateral_for_wrapped(&issued_tokens, vault.data.currency_id)?;

        Ok(required_collateral)
    }

    pub fn vault_exists(id: &T::AccountId) -> bool {
        Vaults::<T>::contains_key(id)
    }

    pub fn compute_collateral(vault_id: &T::AccountId) -> Result<Amount<T>, DispatchError> {
        let collateral = ext::staking::compute_stake::<T>(vault_id, vault_id)?;
        let amount = collateral.try_into().map_err(|_| Error::<T>::TryIntoIntError)?;
        Ok(Amount::new(amount, Self::get_collateral_currency(vault_id)?))
    }

    pub fn get_max_nomination_ratio(currency_id: CurrencyId<T>) -> Result<UnsignedFixedPoint<T>, DispatchError> {
        // MaxNominationRatio = (SecureCollateralThreshold / PremiumRedeemThreshold) - 1)
        // It denotes the maximum amount of collateral that can be nominated to a particular Vault.
        // Its effect is to minimise the impact on collateralization of nominator withdrawals.
        let secure_collateral_threshold =
            Self::secure_collateral_threshold(currency_id).ok_or(Error::<T>::ThresholdNotSet)?;
        let premium_redeem_threshold =
            Self::premium_redeem_threshold(currency_id).ok_or(Error::<T>::ThresholdNotSet)?;
        Ok(secure_collateral_threshold
            .checked_div(&premium_redeem_threshold)
            .ok_or(Error::<T>::ArithmeticUnderflow)?
            .checked_sub(&UnsignedFixedPoint::<T>::one())
            .ok_or(Error::<T>::ArithmeticUnderflow)?)
    }

    pub fn get_max_nominatable_collateral(vault_collateral: &Amount<T>) -> Result<Amount<T>, DispatchError> {
        vault_collateral.rounded_mul(Self::get_max_nomination_ratio(vault_collateral.currency())?)
    }

    /// Private getters and setters

    fn get_collateral_ceiling(currency_id: CurrencyId<T>) -> Result<Amount<T>, DispatchError> {
        let ceiling_amount = SystemCollateralCeiling::<T>::get(currency_id).ok_or(Error::<T>::CeilingNotSet)?;
        Ok(Amount::new(ceiling_amount, currency_id))
    }

    #[cfg_attr(feature = "integration-tests", visibility::make(pub))]
    fn get_total_user_vault_collateral(currency_id: CurrencyId<T>) -> Result<Amount<T>, DispatchError> {
        Ok(Amount::new(
            TotalUserVaultCollateral::<T>::get(currency_id),
            currency_id,
        ))
    }

    fn get_rich_vault_from_id(vault_id: &T::AccountId) -> Result<RichVault<T>, DispatchError> {
        Ok(Self::get_vault_from_id(vault_id)?.into())
    }

    /// Like get_rich_vault_from_id, but only returns active vaults
    fn get_active_rich_vault_from_id(vault_id: &T::AccountId) -> Result<RichVault<T>, DispatchError> {
        Ok(Self::get_active_vault_from_id(vault_id)?.into())
    }

    pub fn get_liquidation_vault(currency_id: CurrencyId<T>) -> DefaultSystemVault<T> {
        if let Some(liquidation_vault) = LiquidationVault::<T>::get(currency_id) {
            liquidation_vault
        } else {
            DefaultSystemVault::<T> {
                to_be_issued_tokens: 0u32.into(),
                issued_tokens: 0u32.into(),
                to_be_redeemed_tokens: 0u32.into(),
                currency_id: currency_id,
            }
        }
    }

    #[cfg_attr(feature = "integration-tests", visibility::make(pub))]
    fn get_rich_liquidation_vault(currency_id: CurrencyId<T>) -> RichSystemVault<T> {
        Self::get_liquidation_vault(currency_id).into()
    }

    fn get_minimum_collateral_vault(currency_id: CurrencyId<T>) -> Amount<T> {
        let amount = MinimumCollateralVault::<T>::get(currency_id);
        Amount::new(amount, currency_id)
    }

    // Other helpers

    /// calculate the collateralization as a ratio of the issued tokens to the
    /// amount of provided collateral at the current exchange rate.
    fn get_collateralization(
        collateral_in_wrapped: &Amount<T>,
        issued_tokens: &Amount<T>,
    ) -> Result<UnsignedFixedPoint<T>, DispatchError> {
        collateral_in_wrapped.ratio(&issued_tokens)
    }

    fn is_vault_below_threshold(
        vault_id: &T::AccountId,
        threshold: UnsignedFixedPoint<T>,
    ) -> Result<bool, DispatchError> {
        let vault = Self::get_rich_vault_from_id(&vault_id)?;

        // the current locked backing collateral by the vault
        let collateral = Self::get_backing_collateral(vault_id)?;

        Self::is_collateral_below_threshold(&collateral, &vault.issued_tokens(), threshold)
    }

    fn is_collateral_below_threshold(
        collateral: &Amount<T>,
        btc_amount: &Amount<T>,
        threshold: UnsignedFixedPoint<T>,
    ) -> Result<bool, DispatchError> {
        let max_tokens = Self::calculate_max_wrapped_from_collateral_for_threshold(collateral, threshold)?;
        // check if the max_tokens are below the issued tokens
        Ok(max_tokens.lt(&btc_amount)?)
    }

    /// Gets the minimum amount of collateral required for the given amount of btc
    /// with the current exchange rate and the given threshold. This function is the
    /// inverse of calculate_max_wrapped_from_collateral_for_threshold
    ///
    /// # Arguments
    /// * `amount_btc` - the amount of wrapped
    /// * `threshold` - the required secure collateral threshold
    fn get_required_collateral_for_wrapped_with_threshold(
        wrapped: &Amount<T>,
        threshold: UnsignedFixedPoint<T>,
        currency_id: CurrencyId<T>,
    ) -> Result<Amount<T>, DispatchError> {
        wrapped
            .checked_fixed_point_mul_rounded_up(&threshold)?
            .convert_to(currency_id)
    }

    fn calculate_max_wrapped_from_collateral_for_threshold(
        collateral: &Amount<T>,
        threshold: UnsignedFixedPoint<T>,
    ) -> Result<Amount<T>, DispatchError> {
        collateral
            .convert_to(T::GetWrappedCurrencyId::get())?
            .checked_div(&threshold)
    }

    pub fn insert_vault_deposit_address(vault_id: &T::AccountId, btc_address: BtcAddress) -> DispatchResult {
        ensure!(
            !ReservedAddresses::<T>::contains_key(&btc_address),
            Error::<T>::ReservedDepositAddress
        );
        let mut vault = Self::get_active_rich_vault_from_id(vault_id)?;
        vault.insert_deposit_address(btc_address);
        ReservedAddresses::<T>::insert(btc_address, vault_id);
        Ok(())
    }

    pub fn new_vault_deposit_address(vault_id: &T::AccountId, secure_id: H256) -> Result<BtcAddress, DispatchError> {
        let mut vault = Self::get_active_rich_vault_from_id(vault_id)?;
        let btc_address = vault.new_deposit_address(secure_id)?;
        Ok(btc_address)
    }

    #[cfg(feature = "integration-tests")]
    pub fn total_user_vault_collateral_integrity_check() {
        for (currency_id, amount) in TotalUserVaultCollateral::<T>::iter() {
            let total_in_vaults = Vaults::<T>::iter()
                .filter_map(|(vault_id, vault)| {
                    if vault.currency_id != currency_id {
                        None
                    } else {
                        Some(Self::get_backing_collateral(&vault_id).unwrap().amount() + vault.liquidated_collateral)
                    }
                })
                .fold(0u32.into(), |acc: BalanceOf<T>, elem| acc + elem);
            let total = total_in_vaults
                + CurrencySource::<T>::LiquidationVault
                    .current_balance(currency_id)
                    .unwrap()
                    .amount();
            assert_eq!(total, amount);
        }
    }
}

trait CheckedMulIntRoundedUp {
    /// Like checked_mul_int, but this version rounds the result up instead of down.
    fn checked_mul_int_rounded_up<N: TryFrom<u128> + TryInto<u128>>(self, n: N) -> Option<N>;
}

impl<T: FixedPointNumber> CheckedMulIntRoundedUp for T {
    fn checked_mul_int_rounded_up<N: TryFrom<u128> + TryInto<u128>>(self, n: N) -> Option<N> {
        // convert n into fixed_point
        let n_inner = TryInto::<T::Inner>::try_into(n.try_into().ok()?).ok()?;
        let n_fixed_point = T::checked_from_integer(n_inner)?;

        // do the multiplication
        let product = self.checked_mul(&n_fixed_point)?;

        // convert to inner
        let product_inner = UniqueSaturatedInto::<u128>::unique_saturated_into(product.into_inner());

        // convert to u128 by dividing by a rounded up division by accuracy
        let accuracy = UniqueSaturatedInto::<u128>::unique_saturated_into(T::accuracy());
        product_inner
            .checked_add(accuracy)?
            .checked_sub(1)?
            .checked_div(accuracy)?
            .try_into()
            .ok()
    }
}
