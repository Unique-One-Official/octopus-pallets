use super::*;
use crate as pallet_octopus_lpos;
use sp_runtime::{
	generic, impl_opaque_keys,
	testing::TestXt,
	traits::{
		AccountIdLookup, BlakeTwo256, ConvertInto, Extrinsic as ExtrinsicT, IdentifyAccount,
		OpaqueKeys, Verify,
	},
	BuildStorage, MultiSignature,
};

pub use frame_support::{
	construct_runtime,
	pallet_prelude::GenesisBuild,
	parameter_types,
	traits::{Hooks, KeyOwnerProofSystem, OnFinalize, OnInitialize, Randomness, StorageInfo},
	weights::{IdentityFee, Weight},
	PalletId, StorageValue,
};

use frame_system::EnsureRoot;

pub(crate) type BlockNumber = u32;
pub type Signature = MultiSignature;
pub type Balance = u128;
pub type Moment = u64;
pub type Index = u64;
pub type Hash = sp_core::H256;

pub const MILLICENTS: Balance = 10_000_000_000;
pub const CENTS: Balance = 1_000 * MILLICENTS;
pub const DOLLARS: Balance = 100 * CENTS;
pub const MILLISECS_PER_BLOCK: Moment = 3000;
pub const SECS_PER_BLOCK: Moment = MILLISECS_PER_BLOCK / 1000;
pub const SLOT_DURATION: Moment = MILLISECS_PER_BLOCK;
pub const EPOCH_DURATION_IN_BLOCKS: BlockNumber = 1 * MINUTES;
pub const MINUTES: BlockNumber = 60 / (SECS_PER_BLOCK as BlockNumber);

parameter_types! {
	pub const BlockHashCount: BlockNumber = 2400;
	pub const SS58Prefix: u16 = 42;
}
impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = ();
	type BlockLength = ();
	type AccountId = AccountId;
	type Call = Call;
	type Lookup = AccountIdLookup<AccountId, ()>;
	type Index = Index;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type Header = generic::Header<BlockNumber, BlakeTwo256>;
	type Event = Event;
	type Origin = Origin;
	type BlockHashCount = BlockHashCount;
	type DbWeight = ();
	type Version = ();
	type PalletInfo = PalletInfo;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type AccountData = pallet_balances::AccountData<Balance>;
	type SystemWeightInfo = ();
	type SS58Prefix = SS58Prefix;
	type OnSetCode = ();
}

parameter_types! {
	pub const MinimumPeriod: Moment = SLOT_DURATION / 2;
}
impl pallet_timestamp::Config for Test {
	type Moment = Moment;
	type OnTimestampSet = ();
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = ();
}

parameter_types! {
	pub const ExistentialDeposit: Balance = 1 * DOLLARS;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}
impl pallet_balances::Config for Test {
	type MaxLocks = MaxLocks;
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
	type Balance = Balance;
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}

use pallet_octopus_appchain::AuthorityId as OctopusId;
impl_opaque_keys! {
	pub struct MockSessionKeys {
		pub octopus: OctopusAppchain,
	}
}

pub struct MockSessionManager;

impl pallet_session::SessionManager<AccountId> for MockSessionManager {
	fn end_session(_: sp_staking::SessionIndex) {}
	fn start_session(index: sp_staking::SessionIndex) {
		OctopusLpos::start_session(index);
	}
	fn new_session(_: sp_staking::SessionIndex) -> Option<Vec<AccountId>> {
		None
	}
}

parameter_types! {
	pub const Period: u32 = 1;
	pub const Offset: u32 = 0;
}
impl pallet_session::Config for Test {
	type Event = Event;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	type ValidatorIdOf = ConvertInto;
	type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
	type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
	type SessionManager = MockSessionManager;
	type SessionHandler = <MockSessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = MockSessionKeys;
	type WeightInfo = pallet_session::weights::SubstrateWeight<Test>;
}

impl pallet_session::historical::Config for Test {
	type FullIdentification = u128;
	type FullIdentificationOf = pallet_octopus_lpos::ExposureOf<Test>;
}

pub(crate) type Extrinsic = TestXt<Call, ()>;
pub(crate) type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

impl frame_system::offchain::SigningTypes for Test {
	type Public = <Signature as Verify>::Signer;
	type Signature = Signature;
}

impl<LocalCall> frame_system::offchain::SendTransactionTypes<LocalCall> for Test
where
	Call: From<LocalCall>,
{
	type OverarchingCall = Call;
	type Extrinsic = Extrinsic;
}

impl<LocalCall> frame_system::offchain::CreateSignedTransaction<LocalCall> for Test
where
	Call: From<LocalCall>,
{
	fn create_transaction<C: frame_system::offchain::AppCrypto<Self::Public, Self::Signature>>(
		call: Call,
		_public: <Signature as Verify>::Signer,
		_account: AccountId,
		nonce: u64,
	) -> Option<(Call, <Extrinsic as ExtrinsicT>::SignaturePayload)> {
		Some((call, (nonce, ())))
	}
}

parameter_types! {
	pub const AssetDeposit: Balance = 100 * DOLLARS;
	pub const ApprovalDeposit: Balance = 1 * DOLLARS;
	pub const StringLimit: u32 = 50;
	pub const MetadataDepositBase: Balance = 10 * DOLLARS;
	pub const MetadataDepositPerByte: Balance = 1 * DOLLARS;
}

impl pallet_assets::Config for Test {
	type Event = Event;
	type Balance = u128;
	type AssetId = u32;
	type Currency = Balances;
	type ForceOrigin = EnsureRoot<AccountId>;
	type AssetDeposit = AssetDeposit;
	type MetadataDepositBase = MetadataDepositBase;
	type MetadataDepositPerByte = MetadataDepositPerByte;
	type ApprovalDeposit = ApprovalDeposit;
	type StringLimit = StringLimit;
	type Freezer = ();
	type Extra = ();
	type WeightInfo = pallet_assets::weights::SubstrateWeight<Test>;
}

pub struct OctopusAppCrypto;

impl frame_system::offchain::AppCrypto<<Signature as Verify>::Signer, Signature>
	for OctopusAppCrypto
{
	type RuntimeAppPublic = pallet_octopus_appchain::AuthorityId;
	type GenericSignature = sp_core::sr25519::Signature;
	type GenericPublic = sp_core::sr25519::Public;
}

impl pallet_octopus_upward_messages::Config for Test {
	type Event = Event;
	type Call = Call;
	type UpwardMessagesLimit = UpwardMessagesLimit;
	type WeightInfo = pallet_octopus_upward_messages::weights::SubstrateWeight<Test>;
}

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;
construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Timestamp: pallet_timestamp::{Pallet, Call, Storage, Inherent},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		OctopusAppchain: pallet_octopus_appchain::{Pallet, Call, Storage, Config<T>, Event<T>, ValidateUnsigned}, // must before session
		OctopusLpos: pallet_octopus_lpos::{Pallet, Call, Config, Storage, Event<T>},
		OctopusUpwardMessages: pallet_octopus_upward_messages::{Pallet, Call, Storage, Event<T>},
		Session: pallet_session::{Pallet, Call, Storage, Event, Config<T>},
		Assets: pallet_assets::{Pallet, Call, Storage, Event<T>, Config<T>},
	}
);

parameter_types! {
	   pub const OctopusAppchainPalletId: PalletId = PalletId(*b"py/octps");
	   pub const GracePeriod: u32 = 10;
	   pub const UnsignedPriority: u64 = 1 << 21;
	   pub const RequestEventLimit: u32 = 10;
	   pub const UpwardMessagesLimit: u32 = 10;
}

impl pallet_octopus_appchain::Config for Test {
	type AuthorityId = OctopusAppCrypto;
	type Event = Event;
	type Call = Call;
	type PalletId = OctopusAppchainPalletId;
	type LposInterface = OctopusLpos;
	type UpwardMessagesInterface = OctopusUpwardMessages;
	type Currency = Balances;
	type Assets = Assets;
	type GracePeriod = GracePeriod;
	type UnsignedPriority = UnsignedPriority;
	type RequestEventLimit = RequestEventLimit;
	type WeightInfo = ();
}

parameter_types! {
	pub const SessionsPerEra: sp_staking::SessionIndex = 6;
	pub const BondingDuration: pallet_octopus_lpos::EraIndex = 24 * 28;
	pub const BlocksPerEra: u32 = EPOCH_DURATION_IN_BLOCKS * 6 / (SECS_PER_BLOCK as u32);
}

impl Config for Test {
	type Currency = Balances;
	type UnixTime = Timestamp;
	type Event = Event;
	type Reward = ();
	type SessionsPerEra = SessionsPerEra;
	type BondingDuration = BondingDuration;
	type BlocksPerEra = BlocksPerEra;
	type SessionInterface = Self;
	type AppchainInterface = OctopusAppchain;
	type UpwardMessagesInterface = OctopusUpwardMessages;
	type PalletId = OctopusAppchainPalletId;
	type ValidatorsProvider = OctopusAppchain;
	type WeightInfo = ();
}

use sp_core::{sr25519, Pair, Public as OtherPublic};
type AccountPublic = <Signature as Verify>::Signer;

pub fn get_account_id_from_seed<TPublic: OtherPublic>(seed: &str) -> AccountId
where
	AccountPublic: From<<TPublic::Pair as Pair>::Public>,
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}
pub fn get_from_seed<TPublic: OtherPublic>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

pub fn authority_keys_from_seed(s: &str) -> (AccountId, OctopusId) {
	(get_account_id_from_seed::<sr25519::Public>(s), get_from_seed::<OctopusId>(s))
}

pub fn new_tester() -> sp_io::TestExternalities {
	let stash: Balance = 100 * 1_000_000_000_000_000_000; // 100 OCT with 18 decimals

	let initial_authorities: Vec<(AccountId, OctopusId)> =
		vec![authority_keys_from_seed("Alice"), authority_keys_from_seed("Bob")];
	let validators = initial_authorities.iter().map(|x| (x.0.clone(), stash)).collect::<Vec<_>>();

	let keys: Vec<_> = initial_authorities
		.iter()
		.map(|x| (x.0.clone(), x.0.clone(), MockSessionKeys { octopus: x.1.clone() }))
		.collect::<Vec<_>>();

	let config =
		pallet_octopus_lpos::GenesisConfig { history_depth: 84u32, era_payout: 2 * DOLLARS };
	let storage = GenesisConfig {
		system: Default::default(),
		octopus_lpos: config,
		assets: Default::default(),
		balances: Default::default(),
		octopus_appchain: pallet_octopus_appchain::GenesisConfig {
			anchor_contract: "oct-test.testnet".to_string(),
			validators,
			premined_amount: 1024 * DOLLARS,
			asset_id_by_name: vec![("usdc.testnet".to_string(), 2)],
		},
		session: pallet_session::GenesisConfig { keys },
	}
	.build_storage()
	.unwrap();

	let mut ext: sp_io::TestExternalities = storage.into();
	ext.execute_with(|| System::set_block_number(1));
	ext
}
