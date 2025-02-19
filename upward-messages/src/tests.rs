use super::*;
use frame_support::{assert_noop, assert_ok, parameter_types};

use sp_core::H256;
use sp_keyring::AccountKeyring;
use sp_runtime::{
	testing::Header,
	traits::{BlakeTwo256, IdentifyAccount, IdentityLookup, Verify},
	MultiSignature,
};

use pallet_octopus_support::types::PayloadType;

use crate as pallet_octopus_upward_messages;

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;
frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Storage, Event<T>},
		OctopusUpwardMessages: pallet_octopus_upward_messages::{Pallet, Call, Storage, Event<T>},
	}
);

pub type Signature = MultiSignature;
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

parameter_types! {
	pub const BlockHashCount: u64 = 250;
}

impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Call = Call;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = ();
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}

parameter_types! {
	pub const UpwardMessagesLimit: u32 = 10;
}

impl Config for Test {
	type Call = Call;
	type Event = Event;
	type UpwardMessagesLimit = UpwardMessagesLimit;
	type WeightInfo = ();
}

pub fn new_tester() -> sp_io::TestExternalities {
	let storage = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
	let mut ext: sp_io::TestExternalities = storage.into();

	ext.execute_with(|| System::set_block_number(1));
	ext
}

#[test]
fn test_submit() {
	new_tester().execute_with(|| {
		let who: AccountId = AccountKeyring::Alice.into();
		assert_ok!(OctopusUpwardMessages::submit(&who, PayloadType::Lock, &vec![0, 1, 2]));
		assert_eq!(<Nonce<Test>>::get(), 1);
		assert_ok!(OctopusUpwardMessages::submit(&who, PayloadType::BurnAsset, &vec![0, 1, 2]));
		assert_eq!(<Nonce<Test>>::get(), 2);
	});
}

#[test]
fn test_submit_exceeds_queue_limit() {
	new_tester().execute_with(|| {
		let who: AccountId = AccountKeyring::Bob.into();

		let messages_limit = UpwardMessagesLimit::get();
		(0..messages_limit).for_each(|_| {
			OctopusUpwardMessages::submit(&who, PayloadType::Lock, &vec![0, 1, 2]).unwrap()
		});

		assert_noop!(
			OctopusUpwardMessages::submit(&who, PayloadType::BurnAsset, &vec![0, 1, 2]),
			Error::<Test>::QueueSizeLimitReached,
		);
	})
}

#[test]
fn test_submit_fails_on_nonce_overflow() {
	new_tester().execute_with(|| {
		let who: AccountId = AccountKeyring::Bob.into();

		<Nonce<Test>>::set(u64::MAX);
		assert_noop!(
			OctopusUpwardMessages::submit(&who, PayloadType::Lock, &vec![0, 1, 2]),
			Error::<Test>::NonceOverflow,
		);
	});
}
