#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::string::{String, ToString};

use borsh::BorshSerialize;
use codec::{Decode, Encode};
use frame_support::{
	traits::{
		tokens::fungibles,
		Currency,
		ExistenceRequirement::{AllowDeath, KeepAlive},
		OneSessionHandler, StorageVersion,
	},
	transactional, PalletId,
};
use frame_system::offchain::{
	AppCrypto, CreateSignedTransaction, SendUnsignedTransaction, SignedPayload, Signer,
	SigningTypes,
};
use pallet_octopus_support::{
	log,
	traits::{AppchainInterface, LposInterface, UpwardMessagesInterface, ValidatorsProvider},
	types::{BurnAssetPayload, LockPayload, PayloadType},
};
use scale_info::TypeInfo;
use serde::{de, Deserialize, Deserializer};
use sp_core::crypto::KeyTypeId;
use sp_runtime::RuntimeAppPublic;
use sp_runtime::{
	offchain::{
		http,
		storage::{MutateStorageError, StorageRetrievalError, StorageValueRef},
		Duration,
	},
	traits::{AccountIdConversion, CheckedConversion, IdentifyAccount, StaticLookup},
	RuntimeDebug,
};
use sp_std::prelude::*;

pub use pallet::*;

pub(crate) const LOG_TARGET: &'static str = "runtime::octopus-appchain";

mod mainchain;
pub mod weights;
pub use weights::WeightInfo;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

/// Defines application identifier for crypto keys of this module.
///
/// Every module that deals with signatures needs to declare its unique identifier for
/// its crypto keys.
/// When offchain worker is signing transactions it's going to request keys of type
/// `KeyTypeId` from the keystore and use the ones it finds to sign the transaction.
/// The keys can be inserted manually via RPC (see `author_insertKey`).
pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"octo");

/// Based on the above `KeyTypeId` we need to generate a pallet-specific crypto type wrappers.
/// We can use from supported crypto kinds (`sr25519`, `ed25519` and `ecdsa`) and augment
/// the types with this pallet-specific identifier.
mod crypto {
	use super::KEY_TYPE;
	use sp_runtime::app_crypto::{app_crypto, sr25519};
	app_crypto!(sr25519, KEY_TYPE);
}

/// Identity of an appchain authority.
pub type AuthorityId = crypto::Public;

type AssetId = u32;
type AssetBalance = u128;

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

type AssetBalanceOf<T> =
	<<T as Config>::Assets as fungibles::Inspect<<T as frame_system::Config>::AccountId>>::Balance;

type AssetIdOf<T> =
	<<T as Config>::Assets as fungibles::Inspect<<T as frame_system::Config>::AccountId>>::AssetId;

/// Validator of appchain.
#[derive(Deserialize, Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct Validator<AccountId> {
	/// The validator's id.
	#[serde(deserialize_with = "deserialize_from_hex_str")]
	#[serde(bound(deserialize = "AccountId: Decode"))]
	validator_id_in_appchain: AccountId,
	/// The total stake of this validator in mainchain's staking system.
	#[serde(deserialize_with = "deserialize_from_str")]
	total_stake: u128,
}

#[derive(Deserialize, Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct ValidatorSet<AccountId> {
	/// The anchor era that this set belongs to.
	set_id: u32,
	/// Validators in this set.
	#[serde(bound(deserialize = "AccountId: Decode"))]
	validators: Vec<Validator<AccountId>>,
}

/// Appchain token burn event.
#[derive(Deserialize, Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct BurnEvent<AccountId> {
	#[serde(default)]
	index: u32,
	#[serde(rename = "sender_id_in_near")]
	#[serde(with = "serde_bytes")]
	sender_id: Vec<u8>,
	#[serde(rename = "receiver_id_in_appchain")]
	#[serde(deserialize_with = "deserialize_from_hex_str")]
	#[serde(bound(deserialize = "AccountId: Decode"))]
	receiver: AccountId,
	#[serde(deserialize_with = "deserialize_from_str")]
	amount: u128,
}

/// Token locked event.
#[derive(Deserialize, Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct LockAssetEvent<AccountId> {
	#[serde(default)]
	index: u32,
	#[serde(rename = "symbol")]
	#[serde(with = "serde_bytes")]
	token_id: Vec<u8>,
	#[serde(rename = "sender_id_in_near")]
	#[serde(with = "serde_bytes")]
	sender_id: Vec<u8>,
	#[serde(rename = "receiver_id_in_appchain")]
	#[serde(deserialize_with = "deserialize_from_hex_str")]
	#[serde(bound(deserialize = "AccountId: Decode"))]
	receiver: AccountId,
	#[serde(deserialize_with = "deserialize_from_str")]
	amount: u128,
}

#[derive(Deserialize, Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum AppchainNotification<AccountId> {
	#[serde(rename = "NearFungibleTokenLocked")]
	#[serde(bound(deserialize = "AccountId: Decode"))]
	LockAsset(LockAssetEvent<AccountId>),

	#[serde(rename = "WrappedAppchainTokenBurnt")]
	#[serde(bound(deserialize = "AccountId: Decode"))]
	Burn(BurnEvent<AccountId>),
}

#[derive(PartialEq, Encode, Decode, Clone, RuntimeDebug, TypeInfo)]
pub enum NotificationResult {
	Success,
	UnlockFailed,
	AssetMintFailed,
	AssetGetFailed,
}

impl Default for NotificationResult {
	fn default() -> Self {
		NotificationResult::Success
	}
}

fn deserialize_from_hex_str<'de, S, D>(deserializer: D) -> Result<S, D::Error>
where
	S: Decode,
	D: Deserializer<'de>,
{
	let account_id_str: String = Deserialize::deserialize(deserializer)?;
	let account_id_hex =
		hex::decode(&account_id_str[2..]).map_err(|e| de::Error::custom(e.to_string()))?;
	S::decode(&mut &account_id_hex[..]).map_err(|e| de::Error::custom(e.to_string()))
}

pub fn deserialize_from_str<'de, S, D>(deserializer: D) -> Result<S, D::Error>
where
	S: sp_std::str::FromStr,
	D: Deserializer<'de>,
	<S as sp_std::str::FromStr>::Err: ToString,
{
	let amount_str: String = Deserialize::deserialize(deserializer)?;
	amount_str.parse::<S>().map_err(|e| de::Error::custom(e.to_string()))
}

#[derive(Deserialize, Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum Observation<AccountId> {
	#[serde(bound(deserialize = "AccountId: Decode"))]
	UpdateValidatorSet(ValidatorSet<AccountId>),
	#[serde(bound(deserialize = "AccountId: Decode"))]
	LockAsset(LockAssetEvent<AccountId>),
	#[serde(bound(deserialize = "AccountId: Decode"))]
	Burn(BurnEvent<AccountId>),
}

#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum ObservationType {
	UpdateValidatorSet,
	Burn,
	LockAsset,
}

impl<AccountId> Observation<AccountId> {
	fn observation_index(&self) -> u32 {
		match self {
			Observation::UpdateValidatorSet(set) => set.set_id,
			Observation::LockAsset(event) => event.index,
			Observation::Burn(event) => event.index,
		}
	}
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct ObservationsPayload<Public, BlockNumber, AccountId> {
	public: Public,
	block_number: BlockNumber,
	observations: Vec<Observation<AccountId>>,
}

impl<T: SigningTypes> SignedPayload<T>
	for ObservationsPayload<T::Public, T::BlockNumber, <T as frame_system::Config>::AccountId>
{
	fn public(&self) -> T::Public {
		self.public.clone()
	}
}

impl<T: Config> AppchainInterface for Pallet<T> {
	fn is_activated() -> bool {
		IsActivated::<T>::get()
	}

	fn next_set_id() -> u32 {
		NextSetId::<T>::get()
	}
}

/// The current storage version.
const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	/// Configure the pallet by specifying the parameters and types on which it depends.
	#[pallet::config]
	pub trait Config: CreateSignedTransaction<Call<Self>> + frame_system::Config {
		/// The identifier type for an offchain worker.
		type AuthorityId: AppCrypto<Self::Public, Self::Signature>;

		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// The overarching dispatch call type.
		type Call: From<Call<Self>>;

		type PalletId: Get<PalletId>;

		type Currency: Currency<Self::AccountId>;

		type Assets: fungibles::Mutate<
			<Self as frame_system::Config>::AccountId,
			AssetId = AssetId,
			Balance = AssetBalance,
		>;

		type LposInterface: LposInterface<Self::AccountId>;
		type UpwardMessagesInterface: UpwardMessagesInterface<Self::AccountId>;

		// Configuration parameters

		/// A grace period after we send transaction.
		///
		/// To avoid sending too many transactions, we only attempt to send one
		/// every `GRACE_PERIOD` blocks. We use Local Storage to coordinate
		/// sending between distinct runs of this offchain worker.
		#[pallet::constant]
		type GracePeriod: Get<Self::BlockNumber>;

		/// A configuration for base priority of unsigned transactions.
		///
		/// This is exposed so that it can be tuned for particular runtime, when
		/// multiple pallets send unsigned transactions.
		#[pallet::constant]
		type UnsignedPriority: Get<TransactionPriority>;

		#[pallet::constant]
		type RequestEventLimit: Get<u32>;

		type WeightInfo: WeightInfo;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::type_value]
	pub(super) fn DefaultForAnchorContract() -> Vec<u8> {
		Vec::new()
	}

	#[pallet::storage]
	#[pallet::getter(fn anchor_contract)]
	pub(super) type AnchorContract<T: Config> =
		StorageValue<_, Vec<u8>, ValueQuery, DefaultForAnchorContract>;

	/// Whether the appchain is activated.
	///
	/// Only an active appchain will communicate with the mainchain and pay block rewards.
	#[pallet::storage]
	pub(super) type IsActivated<T: Config> = StorageValue<_, bool, ValueQuery>;

	#[pallet::storage]
	pub type NextSetId<T: Config> = StorageValue<_, u32, ValueQuery>;

	#[pallet::storage]
	pub type PlannedValidators<T: Config> = StorageValue<_, Vec<(T::AccountId, u128)>, ValueQuery>;

	#[pallet::storage]
	pub type NextNotificationId<T: Config> = StorageValue<_, u32, ValueQuery>;

	#[pallet::storage]
	pub type Observations<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		ObservationType,
		Twox64Concat,
		u32,
		Vec<Observation<T::AccountId>>,
		ValueQuery,
	>;

	#[pallet::storage]
	pub type Observing<T: Config> =
		StorageMap<_, Twox64Concat, Observation<T::AccountId>, Vec<T::AccountId>, ValueQuery>;

	#[pallet::storage]
	pub type AssetIdByName<T: Config> =
		StorageMap<_, Twox64Concat, Vec<u8>, AssetIdOf<T>, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn pallet_account)]
	pub type PalletAccount<T: Config> = StorageValue<_, T::AccountId, ValueQuery>;

	#[pallet::storage]
	pub type NotificationHistory<T: Config> =
		StorageMap<_, Twox64Concat, u32, NotificationResult, ValueQuery>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub anchor_contract: String,
		pub validators: Vec<(T::AccountId, u128)>,
		pub premined_amount: u128,
		pub asset_id_by_name: Vec<(String, AssetIdOf<T>)>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				anchor_contract: String::new(),
				validators: Vec::new(),
				premined_amount: 0,
				asset_id_by_name: Vec::new(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			<AnchorContract<T>>::put(self.anchor_contract.as_bytes());

			<NextSetId<T>>::put(1); // set 0 is already in the genesis
			<PlannedValidators<T>>::put(self.validators.clone());

			let account_id = <Pallet<T>>::account_id();
			let min = T::Currency::minimum_balance();
			let amount =
				self.premined_amount.checked_into().ok_or(Error::<T>::AmountOverflow).unwrap();
			if amount >= min {
				T::Currency::make_free_balance_be(&account_id, amount);
			}

			<PalletAccount<T>>::put(account_id);

			for (token_id, id) in self.asset_id_by_name.iter() {
				<AssetIdByName<T>>::insert(token_id.as_bytes(), id);
			}
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		Locked(T::AccountId, Vec<u8>, BalanceOf<T>),
		Unlocked(Vec<u8>, T::AccountId, BalanceOf<T>),
		AssetMinted(AssetIdOf<T>, Vec<u8>, T::AccountId, AssetBalanceOf<T>),
		AssetBurned(AssetIdOf<T>, T::AccountId, Vec<u8>, AssetBalanceOf<T>),
		UnlockFailed(Vec<u8>, T::AccountId, BalanceOf<T>),
		AssetMintFailed(AssetIdOf<T>, Vec<u8>, T::AccountId, AssetBalanceOf<T>),
		AssetIdGetFailed(Vec<u8>, Vec<u8>, T::AccountId, AssetBalanceOf<T>),
	}

	// Errors inform users that something went wrong.
	#[pallet::error]
	pub enum Error<T> {
		/// The set id of new validator set was wrong.
		WrongSetId,
		/// Invalid notification id of observation.
		InvalidNotificationId,
		/// Must be a validator.
		NotValidator,
		/// Amount overflow.
		AmountOverflow,
		/// Next notification Id overflow.
		NextNotificationIdOverflow,
		/// Wrong Asset Id.
		WrongAssetId,
		/// Invalid active total stake.
		InvalidActiveTotalStake,
		/// Appchain is not activated.
		NotActivated,
		/// ReceiverId is not a valid utf8 string.
		InvalidReceiverId,
		/// Token is not a valid utf8 string.
		InvalidTokenId,
		/// Next set Id overflow.
		NextSetIdOverflow,
		/// Observations exceeded limit.
		ObservationsExceededLimit,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		/// Offchain Worker entry point.
		///
		/// By implementing `fn offchain_worker` you declare a new offchain worker.
		/// This function will be called when the node is fully synced and a new best block is
		/// succesfuly imported.
		/// Note that it's not guaranteed for offchain workers to run on EVERY block, there might
		/// be cases where some blocks are skipped, or for some the worker runs twice (re-orgs),
		/// so the code should be able to handle that.
		/// You can use `Local Storage` API to coordinate runs of the worker.
		fn offchain_worker(block_number: T::BlockNumber) {
			let anchor_contract = Self::anchor_contract();
			if !sp_io::offchain::is_validator()
				|| !IsActivated::<T>::get()
				|| anchor_contract.is_empty()
			{
				return;
			}

			let parent_hash = <frame_system::Pallet<T>>::block_hash(block_number - 1u32.into());
			log!(debug, "Current block: {:?} (parent hash: {:?})", block_number, parent_hash);

			if !Self::should_send(block_number) {
				return;
			}

			// Only communicate with mainchain if we are validators.
			match Self::get_validator_id() {
				Some((public, validator_id)) => {
					log!(debug, "public: {:?}, validator_id: {:?}", public, validator_id);

					let mainchain_rpc_endpoint = Self::get_mainchain_rpc_endpoint(
						anchor_contract[anchor_contract.len() - 1] == 116,
					); // last byte is 't'
					log!(debug, "current mainchain_rpc_endpoint {:?}", mainchain_rpc_endpoint);

					if let Err(e) = Self::observing_mainchain(
						block_number,
						&mainchain_rpc_endpoint,
						anchor_contract,
						public,
						validator_id,
					) {
						log!(warn, "observing_mainchain: Error: {}", e);
					}
				}
				None => {
					log!(warn, "Not a validator, skipping offchain worker");
				}
			}
		}
	}

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T> {
		type Call = Call<T>;

		/// Validate unsigned call to this module.
		///
		/// By default unsigned transactions are disallowed, but implementing the validator
		/// here we make sure that some particular calls (the ones produced by offchain worker)
		/// are being whitelisted and marked as valid.
		fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
			// Firstly let's check that we call the right function.
			if let Call::submit_observations { ref payload, ref signature } = call {
				let signature_valid =
					SignedPayload::<T>::verify::<T::AuthorityId>(payload, signature.clone());
				if !signature_valid {
					return InvalidTransaction::BadProof.into();
				}
				Self::validate_transaction_parameters(
					&payload.block_number,
					payload.public.clone().into_account(),
				)
			} else {
				InvalidTransaction::Call.into()
			}
		}
	}

	// Dispatchable functions allows users to interact with the pallet and invoke state changes.
	// These functions materialize as "extrinsics", which are often compared to transactions.
	// Dispatchable functions must be annotated with a weight and must return a DispatchResult.
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Submit observations.
		#[pallet::weight(0)]
		pub fn submit_observations(
			origin: OriginFor<T>,
			payload: ObservationsPayload<
				T::Public,
				T::BlockNumber,
				<T as frame_system::Config>::AccountId,
			>,
			_signature: T::Signature,
		) -> DispatchResultWithPostInfo {
			// This ensures that the function can only be called via unsigned transaction.
			ensure_none(origin)?;
			let who = payload.public.clone().into_account();
			let val_id = T::LposInterface::is_active_validator(
				KEY_TYPE,
				&payload.public.clone().into_account().encode(),
			);

			if val_id.is_none() {
				log!(
					warn,
					"Not a validator in current validator set: {:?}",
					payload.public.clone().into_account()
				);
				return Err(Error::<T>::NotValidator.into());
			}
			let val_id = val_id.expect("Validator is valid; qed").clone();

			//
			log!(debug, "️️️observations: {:#?},\nwho: {:?}", payload.observations, who);
			//

			for observation in payload.observations.iter() {
				if let Err(e) = Self::submit_observation(&val_id, observation.clone()) {
					log!(warn, "OCTOPUS-ALERT-DISCORD submit_observation: Error: {:?}", e);
				}
			}

			Ok(().into())
		}

		#[pallet::weight(<T as Config>::WeightInfo::force_set_is_activated())]
		pub fn force_set_is_activated(origin: OriginFor<T>, is_activated: bool) -> DispatchResult {
			ensure_root(origin)?;
			<IsActivated<T>>::put(is_activated);
			Ok(())
		}

		#[pallet::weight(<T as Config>::WeightInfo::force_set_next_set_id(*next_set_id))]
		pub fn force_set_next_set_id(origin: OriginFor<T>, next_set_id: u32) -> DispatchResult {
			ensure_root(origin)?;
			<NextSetId<T>>::put(next_set_id);
			Ok(())
		}

		// Force set planned validators with sudo permissions.
		#[pallet::weight(<T as Config>::WeightInfo::force_set_planned_validators(validators.len() as u32))]
		pub fn force_set_planned_validators(
			origin: OriginFor<T>,
			validators: Vec<(T::AccountId, u128)>,
		) -> DispatchResult {
			ensure_root(origin)?;
			<PlannedValidators<T>>::put(validators);
			Ok(())
		}

		// cross chain transfer

		// There are 2 kinds of assets:
		// 1. native token on appchain
		// mainchain:mint() <- appchain:lock()
		// mainchain:burn() -> appchain:unlock()
		//
		// 2. NEP141 asset on mainchain
		// mainchain:lock_asset()   -> appchain:mint_asset()
		// mainchain:unlock_asset() <- appchain:burn_asset()

		#[pallet::weight(<T as Config>::WeightInfo::lock())]
		#[transactional]
		pub fn lock(
			origin: OriginFor<T>,
			receiver_id: Vec<u8>,
			amount: BalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;
			ensure!(IsActivated::<T>::get(), Error::<T>::NotActivated);

			let receiver_id =
				String::from_utf8(receiver_id).map_err(|_| Error::<T>::InvalidReceiverId)?;

			let amount_wrapped: u128 = amount.checked_into().ok_or(Error::<T>::AmountOverflow)?;

			T::Currency::transfer(&who, &Self::account_id(), amount, AllowDeath)?;

			let prefix = String::from("0x");
			let hex_sender = prefix + &hex::encode(who.encode());
			let message = LockPayload {
				sender: hex_sender.clone(),
				receiver_id: receiver_id.clone(),
				amount: amount_wrapped,
			};

			T::UpwardMessagesInterface::submit(
				&who,
				PayloadType::Lock,
				&message.try_to_vec().unwrap(),
			)?;
			Self::deposit_event(Event::Locked(who, receiver_id.as_bytes().to_vec(), amount));

			Ok(().into())
		}

		#[pallet::weight(0)]
		#[transactional]
		pub fn mint_asset(
			origin: OriginFor<T>,
			asset_id: AssetIdOf<T>,
			sender_id: Vec<u8>,
			receiver: <T::Lookup as StaticLookup>::Source,
			amount: AssetBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			ensure_root(origin)?;

			let receiver = T::Lookup::lookup(receiver)?;
			Self::mint_asset_inner(asset_id, sender_id, receiver, amount)
		}

		#[pallet::weight(0)]
		#[transactional]
		pub fn burn_asset(
			origin: OriginFor<T>,
			asset_id: AssetIdOf<T>,
			receiver_id: Vec<u8>,
			amount: AssetBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			let sender = ensure_signed(origin)?;
			ensure!(IsActivated::<T>::get(), Error::<T>::NotActivated);

			let receiver_id =
				String::from_utf8(receiver_id).map_err(|_| Error::<T>::InvalidReceiverId)?;

			let token_id = <AssetIdByName<T>>::iter()
				.find(|p| p.1 == asset_id)
				.map(|p| p.0)
				.ok_or(Error::<T>::WrongAssetId)?;

			let token_id = String::from_utf8(token_id).map_err(|_| Error::<T>::InvalidTokenId)?;

			<T::Assets as fungibles::Mutate<T::AccountId>>::burn_from(asset_id, &sender, amount)?;

			let prefix = String::from("0x");
			let hex_sender = prefix + &hex::encode(sender.encode());
			let message = BurnAssetPayload {
				token_id,
				sender: hex_sender,
				receiver_id: receiver_id.clone(),
				amount,
			};

			T::UpwardMessagesInterface::submit(
				&sender,
				PayloadType::BurnAsset,
				&message.try_to_vec().unwrap(),
			)?;
			Self::deposit_event(Event::AssetBurned(
				asset_id,
				sender,
				receiver_id.as_bytes().to_vec(),
				amount,
			));

			Ok(().into())
		}
	}

	impl<T: Config> Pallet<T> {
		fn account_id() -> T::AccountId {
			T::PalletId::get().into_account()
		}

		fn bsngate_rpc_endpoint(is_testnet: bool) -> String {
			if is_testnet {
				"https://ca.bsngate.com/api/8803b555a830c4d2ac680a7fdefc46aeb7738c4f6f0513f0aec328768ad71002/Near-Testnet/rpc".to_string()
			} else {
				"https://ca.bsngate.com/api/edc6aab2f13e1dc049fab8b4bcae29cdae53ce84df2d8b352f9497f290a697e2/Near-Mainnet/rpc".to_string()
			}
		}

		fn default_rpc_endpoint(is_testnet: bool) -> String {
			if is_testnet {
				"https://rpc.testnet.near.org".to_string()
			} else {
				"https://rpc.mainnet.near.org".to_string()
			}
		}

		fn get_mainchain_rpc_endpoint(is_testnet: bool) -> String {
			let kind = sp_core::offchain::StorageKind::PERSISTENT;
			if let Some(data) = sp_io::offchain::local_storage_get(
				kind,
				b"octopus_appchain::mainchain_rpc_endpoint",
			) {
				if let Ok(rpc_url) = String::from_utf8(data) {
					log!(debug, "The configure url is {:?} ", rpc_url.clone());
					return rpc_url;
				} else {
					log!(warn, "Parse configure url error, return default rpc url");
					return Self::default_rpc_endpoint(is_testnet);
				}
			} else {
				log!(debug, "No configuration for rpc, return default rpc url");
				return Self::default_rpc_endpoint(is_testnet);
			}
		}

		fn should_send(block_number: T::BlockNumber) -> bool {
			/// A friendlier name for the error that is going to be returned in case we are in the grace
			/// period.
			const RECENTLY_SENT: () = ();

			// Start off by creating a reference to Local Storage value.
			// Since the local storage is common for all offchain workers, it's a good practice
			// to prepend your entry with the module name.
			let val = StorageValueRef::persistent(b"octopus_appchain::last_send");
			// The Local Storage is persisted and shared between runs of the offchain workers,
			// and offchain workers may run concurrently. We can use the `mutate` function, to
			// write a storage entry in an atomic fashion. Under the hood it uses `compare_and_set`
			// low-level method of local storage API, which means that only one worker
			// will be able to "acquire a lock" and send a transaction if multiple workers
			// happen to be executed concurrently.
			let res =
				val.mutate(|last_send: Result<Option<T::BlockNumber>, StorageRetrievalError>| {
					match last_send {
						// If we already have a value in storage and the block number is recent enough
						// we avoid sending another transaction at this time.
						Ok(Some(block)) if block_number < block + T::GracePeriod::get() => {
							Err(RECENTLY_SENT)
						}
						// In every other case we attempt to acquire the lock and send a transaction.
						_ => Ok(block_number),
					}
				});

			match res {
				// The value has been set correctly, which means we can safely send a transaction now.
				Ok(_) => true,
				// We are in the grace period, we should not send a transaction this time.
				Err(MutateStorageError::ValueFunctionFailed(RECENTLY_SENT)) => false,
				// We wanted to send a transaction, but failed to write the block number (acquire a
				// lock). This indicates that another offchain worker that was running concurrently
				// most likely executed the same logic and succeeded at writing to storage.
				// Thus we don't really want to send the transaction, knowing that the other run
				// already did.
				Err(MutateStorageError::ConcurrentModification(_)) => false,
			}
		}

		fn get_validator_id() -> Option<(<T as SigningTypes>::Public, T::AccountId)> {
			for key in <T::AuthorityId as AppCrypto<
				<T as SigningTypes>::Public,
				<T as SigningTypes>::Signature,
			>>::RuntimeAppPublic::all()
			.into_iter()
			{
				let generic_public = <T::AuthorityId as AppCrypto<
					<T as SigningTypes>::Public,
					<T as SigningTypes>::Signature,
				>>::GenericPublic::from(key);
				let public: <T as SigningTypes>::Public = generic_public.into();

				let val_id = T::LposInterface::is_active_validator(
					KEY_TYPE,
					&public.clone().into_account().encode(),
				);

				if val_id.is_none() {
					continue;
				}
				return Some((public, val_id.unwrap()));
			}
			None
		}

		pub(crate) fn observing_mainchain(
			block_number: T::BlockNumber,
			mainchain_rpc_endpoint: &str,
			anchor_contract: Vec<u8>,
			public: <T as SigningTypes>::Public,
			_validator_id: T::AccountId,
		) -> Result<(), &'static str> {
			let mut obs: Vec<Observation<<T as frame_system::Config>::AccountId>>;
			let next_notification_id = NextNotificationId::<T>::get();
			log!(debug, "next_notification_id: {}", next_notification_id);
			let next_set_id = NextSetId::<T>::get();
			log!(debug, "next_set_id: {}", next_set_id);

			// Make an external HTTP request to fetch the current price.
			// Note this call will block until response is received.
			let ret = Self::get_validator_list_of(
				mainchain_rpc_endpoint,
				anchor_contract.clone(),
				next_set_id,
			);

			match ret {
				Ok(observations) => {
					obs = observations;
				}
				Err(_) => {
					log!(debug, "retry with failsafe endpoint to get validators");
					obs = Self::get_validator_list_of(
						&Self::bsngate_rpc_endpoint(
							anchor_contract[anchor_contract.len() - 1] == 116,
						), // last byte is 't'
						anchor_contract.clone(),
						next_set_id,
					)
					.map_err(|_| "Failed to get_validator_list_of")?;
				}
			}

			// check cross-chain transfers only if there isn't a validator_set update.
			if obs.len() == 0 {
				log!(debug, "No validat_set updates, try to get appchain notifications.");
				// Make an external HTTP request to fetch the current price.
				// Note this call will block until response is received.
				let ret = Self::get_appchain_notification_histories(
					mainchain_rpc_endpoint,
					anchor_contract.clone(),
					next_notification_id,
					T::RequestEventLimit::get(),
				);

				match ret {
					Ok(observations) => {
						obs = observations;
					}
					Err(_) => {
						log!(debug, "retry with failsafe endpoint to get notify");
						obs = Self::get_appchain_notification_histories(
							&Self::bsngate_rpc_endpoint(
								anchor_contract[anchor_contract.len() - 1] == 116,
							), // last byte is 't'
							anchor_contract,
							next_notification_id,
							T::RequestEventLimit::get(),
						)
						.map_err(|_| "Failed to get_appchain_notification_histories")?;
					}
				}
			}

			if obs.len() == 0 {
				log!(debug, "No messages from mainchain.");
				return Ok(());
			}

			let result = Signer::<T, T::AuthorityId>::all_accounts()
				.with_filter(vec![public])
				.send_unsigned_transaction(
					|account| ObservationsPayload {
						public: account.public.clone(),
						block_number,
						observations: obs.clone(),
					},
					|payload, signature| Call::submit_observations { payload, signature },
				);
			if result.len() != 1 {
				return Err("No account found");
			}
			if result[0].1.is_err() {
				log!(
					warn,
					"OCTOPUS-ALERT-DISCORD Failed to submit observations: {:?}",
					result[0].1
				);

				return Err("Failed to submit observations");
			}

			Ok(())
		}

		fn unlock_inner(
			sender_id: Vec<u8>,
			receiver: T::AccountId,
			amount: u128,
		) -> DispatchResultWithPostInfo {
			let amount_unwrapped = amount.checked_into().ok_or(Error::<T>::AmountOverflow)?;
			// unlock native token
			T::Currency::transfer(&Self::account_id(), &receiver, amount_unwrapped, KeepAlive)?;
			Self::deposit_event(Event::Unlocked(sender_id, receiver, amount_unwrapped));

			Ok(().into())
		}

		fn mint_asset_inner(
			asset_id: AssetIdOf<T>,
			sender_id: Vec<u8>,
			receiver: T::AccountId,
			amount: AssetBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			<T::Assets as fungibles::Mutate<T::AccountId>>::mint_into(asset_id, &receiver, amount)?;
			Self::deposit_event(Event::AssetMinted(asset_id, sender_id, receiver, amount));

			Ok(().into())
		}

		fn increase_next_notification_id() -> DispatchResultWithPostInfo {
			NextNotificationId::<T>::try_mutate(|next_id| -> DispatchResultWithPostInfo {
				if let Some(v) = next_id.checked_add(1) {
					*next_id = v;
					log!(debug, "️️️increase next_notification_id{:?} ", v);
				} else {
					return Err(Error::<T>::NextNotificationIdOverflow.into());
				}
				Ok(().into())
			})
		}

		fn increase_next_set_id() -> DispatchResultWithPostInfo {
			NextSetId::<T>::try_mutate(|next_id| -> DispatchResultWithPostInfo {
				if let Some(v) = next_id.checked_add(1) {
					*next_id = v;
				} else {
					return Err(Error::<T>::NextSetIdOverflow.into());
				}
				Ok(().into())
			})
		}

		fn check_observation(
			observation_type: ObservationType,
			obs_id: u32,
		) -> DispatchResultWithPostInfo {
			match observation_type {
				ObservationType::UpdateValidatorSet => {
					let next_set_id = NextSetId::<T>::get();
					if obs_id != next_set_id {
						log!(
							warn,
							"wrong set id for update validator set: {:?}, expected: {:?}",
							obs_id,
							next_set_id
						);
						return Err(Error::<T>::WrongSetId.into());
					}
				}
				_ => {
					let next_notification_id = NextNotificationId::<T>::get();
					let limit = T::RequestEventLimit::get();
					if (obs_id < next_notification_id) || (obs_id >= next_notification_id + limit) {
						log!(
							warn,
							"invalid notification id for observation: {:?}, expected: [{:?}, {:?})",
							obs_id,
							next_notification_id,
							next_notification_id + limit
						);
						return Err(Error::<T>::InvalidNotificationId.into());
					}
				}
			}

			// The maximum number of observation for the same obs_id is the number of validators (100),
			// that is, each validator submits a observation.
			let obs = <Observations<T>>::try_get(observation_type, obs_id);
			if let Ok(obs) = obs {
				if obs.len() > 100 {
					log!(
						warn,
						"the number of observations with ({:?}, {:?}) exceeded the upper limit",
						obs_id,
						observation_type
					);
					return Err(Error::<T>::ObservationsExceededLimit.into());
				}
			}

			Ok(().into())
		}

		fn prune_old_histories() {
			// let next_notification_id = NextNotificationId::<T>::get();
			// if next_notification_id <= T::NotificationHistoryDepth::get() {
			// 	return;
			// }

			// let prune_index = next_notification_id - T::NotificationHistoryDepth::get();

			// // prune observations
			// let prune_obs = <Observations<T>>::iter_prefix(observation_type)
			// 	.filter(|(index, _)| *index < prune_index)
			// 	.collect::<Vec<(u32, Vec<Observation<T::AccountId>>)>>();

			// log!(debug, "will delete old observations: {:#?}", prune_obs.clone());
			// let _ = prune_obs
			// 	.iter()
			// 	.map(|(index, obs)| {
			// 		for o in obs.iter() {
			// 			<Observing<T>>::remove(o);
			// 		}
			// 		<Observations<T>>::remove(observation_type, index);
			// 	})
			// 	.collect::<Vec<_>>();

			// // prune records
			// // TODO: use simple code
			// let prune_records = <ObservationRecords<T>>::iter_prefix(record_type)
			// 	.filter(|(index, _)| *index <= prune_index)
			// 	.collect::<Vec<(u32, ObservationRecord<T::AccountId>)>>();

			// log!(debug, "will delete old records: {:#?}", prune_records.clone());
			// let _ = prune_records
			// 	.iter()
			// 	.map(|(index, _)| {
			// 		<ObservationRecords<T>>::remove(record_type, index);
			// 	})
			// 	.collect::<Vec<_>>();
		}

		/// If the observation already exists in the Observations, then the only thing
		/// to do is vote for this observation.
		#[transactional]
		fn submit_observation(
			validator_id: &T::AccountId,
			observation: Observation<T::AccountId>,
		) -> DispatchResultWithPostInfo {
			let observation_type = Self::get_observation_type(&observation);
			let obs_id = observation.observation_index();
			Self::check_observation(observation_type, obs_id)?;

			<Observations<T>>::mutate(observation_type, obs_id, |obs| {
				let found = obs.iter().any(|o| o == &observation);
				if !found {
					obs.push(observation.clone())
				}
			});
			<Observing<T>>::mutate(&observation, |vals| {
				let found = vals.iter().any(|id| id == validator_id);
				if !found {
					vals.push(validator_id.clone());
				} else {
					log!(warn, "{:?} submits a duplicate ocw tx", validator_id);
				}
			});
			let total_stake: u128 = T::LposInterface::active_total_stake()
				.ok_or(Error::<T>::InvalidActiveTotalStake)?;
			let stake: u128 = <Observing<T>>::get(&observation)
				.iter()
				.map(|v| T::LposInterface::active_stake_of(v))
				.sum();

			//
			log!(debug, "observations type: {:#?}", observation_type);
			log!(
				debug,
				"️️️observations content: {:#?}",
				<Observations<T>>::get(observation_type, obs_id)
			);
			log!(debug, "️️️observer: {:#?}", <Observing<T>>::get(&observation));
			log!(debug, "️️️total_stake: {:?}, stake: {:?}", total_stake, stake);
			//

			if 3 * stake > 2 * total_stake {
				match observation.clone() {
					Observation::UpdateValidatorSet(val_set) => {
						let validators: Vec<(T::AccountId, u128)> = val_set
							.validators
							.iter()
							.map(|v| (v.validator_id_in_appchain.clone(), v.total_stake))
							.collect();
						<PlannedValidators<T>>::put(validators.clone());
						log!(debug, "new PlannedValidators: {:?}", validators);
						Self::increase_next_set_id()?;
					}
					Observation::Burn(event) => {
						Self::increase_next_notification_id()?;
						let mut result = NotificationResult::Success;
						if let Err(error) = Self::unlock_inner(
							event.sender_id.clone(),
							event.receiver.clone(),
							event.amount,
						) {
							log!(info, "️️️failed to unlock native token: {:?}", error);
							let min = T::Currency::minimum_balance();
							let amount_unwrapped = event.amount.checked_into().unwrap_or(min); //Check: should not return error.
							Self::deposit_event(Event::UnlockFailed(
								event.sender_id,
								event.receiver,
								amount_unwrapped,
							));
							result = NotificationResult::UnlockFailed;
						}
						NotificationHistory::<T>::insert(obs_id, result.clone());
						log!(
							debug,
							"save notification result {:?}:{:?} to NotificationHistory ",
							obs_id,
							result
						);
					}
					Observation::LockAsset(event) => {
						Self::increase_next_notification_id()?;
						let mut result = NotificationResult::Success;
						if let Ok(asset_id) = <AssetIdByName<T>>::try_get(&event.token_id) {
							log!(
								info,
								"️️️mint asset:{:?}, sender_id:{:?}, receiver:{:?}, amount:{:?}",
								asset_id,
								event.sender_id,
								event.receiver,
								event.amount,
							);
							if let Err(error) = Self::mint_asset_inner(
								asset_id,
								event.sender_id.clone(),
								event.receiver.clone(),
								event.amount,
							) {
								log!(warn, "️️️failed to mint asset: {:?}", error);
								Self::deposit_event(Event::AssetMintFailed(
									asset_id,
									event.sender_id,
									event.receiver,
									event.amount,
								));
								result = NotificationResult::AssetMintFailed;
							}
						} else {
							Self::deposit_event(Event::AssetIdGetFailed(
								event.token_id,
								event.sender_id,
								event.receiver,
								event.amount,
							));
							result = NotificationResult::AssetGetFailed;
						}

						NotificationHistory::<T>::insert(obs_id, result.clone());
						log!(
							debug,
							"save notification result {:?}:{:?} to NotificationHistory ",
							obs_id,
							result
						);
					}
				}

				Self::prune_old_histories();
			}

			Ok(().into())
		}

		fn get_observation_type(observation: &Observation<T::AccountId>) -> ObservationType {
			match observation.clone() {
				Observation::UpdateValidatorSet(_) => {
					return ObservationType::UpdateValidatorSet;
				}
				Observation::Burn(_) => {
					return ObservationType::Burn;
				}
				Observation::LockAsset(_) => {
					return ObservationType::LockAsset;
				}
			}
		}

		fn validate_transaction_parameters(
			block_number: &T::BlockNumber,
			account_id: <T as frame_system::Config>::AccountId,
		) -> TransactionValidity {
			// Let's make sure to reject transactions from the future.
			let current_block = <frame_system::Pallet<T>>::block_number();
			if &current_block < block_number {
				log!(
					warn,
					"InvalidTransaction => current_block: {:?}, block_number: {:?}",
					current_block,
					block_number
				);
				return InvalidTransaction::Future.into();
			}

			ValidTransaction::with_tag_prefix("OctopusAppchain")
				// We set base priority to 2**21 and hope it's included before any other
				// transactions in the pool.
				.priority(T::UnsignedPriority::get())
				// This transaction does not require anything else to go before into the pool.
				//.and_requires()
				// We set the `provides` tag to `account_id`. This makes
				// sure only one transaction produced by current validator will ever
				// get to the transaction pool and will end up in the block.
				// We can still have multiple transactions compete for the same "spot",
				// and the one with higher priority will replace other one in the pool.
				.and_provides(account_id)
				// The transaction is only valid for next 5 blocks. After that it's
				// going to be revalidated by the pool.
				.longevity(5)
				// It's fine to propagate that transaction to other peers, which means it can be
				// created even by nodes that don't produce blocks.
				// Note that sometimes it's better to keep it for yourself (if you are the block
				// producer), since for instance in some schemes others may copy your solution and
				// claim a reward.
				.propagate(true)
				.build()
		}
	}

	impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Pallet<T> {
		type Public = AuthorityId;
	}

	impl<T: Config> OneSessionHandler<T::AccountId> for Pallet<T> {
		type Key = AuthorityId;

		fn on_genesis_session<'a, I: 'a>(_authorities: I)
		where
			I: Iterator<Item = (&'a T::AccountId, Self::Key)>,
		{
			// ignore
		}

		fn on_new_session<'a, I: 'a>(_changed: bool, _validators: I, _queued_validators: I)
		where
			I: Iterator<Item = (&'a T::AccountId, Self::Key)>,
		{
			// ignore
		}

		fn on_disabled(_i: u32) {
			// ignore
		}
	}

	impl<T: Config> ValidatorsProvider<T::AccountId> for Pallet<T> {
		fn validators() -> Vec<(T::AccountId, u128)> {
			<PlannedValidators<T>>::get()
		}
	}
}
