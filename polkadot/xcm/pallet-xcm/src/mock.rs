// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use codec::Encode;
use frame_support::{
	construct_runtime, match_types, parameter_types,
	traits::{ConstU32, Everything, EverythingBut, Nothing},
	weights::Weight,
};
use frame_system::EnsureRoot;
use polkadot_parachain_primitives::primitives::Id as ParaId;
use polkadot_runtime_parachains::origin;
use sp_core::H256;
use sp_runtime::{traits::IdentityLookup, AccountId32, BuildStorage};
pub use sp_std::{
	cell::RefCell, collections::btree_map::BTreeMap, fmt::Debug, marker::PhantomData,
};
use xcm::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowKnownQueryResponses, AllowSubscriptionsFrom,
	AllowTopLevelPaidExecutionFrom, Case, ChildParachainAsNative, ChildParachainConvertsVia,
	ChildSystemParachainAsSuperuser, CurrencyAdapter as XcmCurrencyAdapter, FixedRateOfFungible,
	FixedWeightBounds, IsConcrete, SignedAccountId32AsNative, SignedToAccountId32,
	SovereignSignedViaLocation, TakeWeightCredit, XcmFeeManagerFromComponents, XcmFeeToAccount,
};
use xcm_executor::XcmExecutor;

use crate::{self as pallet_xcm, TestWeightInfo};

pub type AccountId = AccountId32;
pub type Balance = u128;
type Block = frame_system::mocking::MockBlock<Test>;

#[frame_support::pallet]
pub mod pallet_test_notifier {
	use crate::{ensure_response, QueryId};
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use sp_runtime::DispatchResult;
	use xcm::latest::prelude::*;
	use xcm_executor::traits::QueryHandler;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + crate::Config {
		type RuntimeEvent: IsType<<Self as frame_system::Config>::RuntimeEvent> + From<Event<Self>>;
		type RuntimeOrigin: IsType<<Self as frame_system::Config>::RuntimeOrigin>
			+ Into<Result<crate::Origin, <Self as Config>::RuntimeOrigin>>;
		type RuntimeCall: IsType<<Self as crate::Config>::RuntimeCall> + From<Call<Self>>;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		QueryPrepared(QueryId),
		NotifyQueryPrepared(QueryId),
		ResponseReceived(MultiLocation, QueryId, Response),
	}

	#[pallet::error]
	pub enum Error<T> {
		UnexpectedId,
		BadAccountFormat,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::call_index(0)]
		#[pallet::weight(Weight::from_parts(1_000_000, 1_000_000))]
		pub fn prepare_new_query(origin: OriginFor<T>, querier: MultiLocation) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let id = who
				.using_encoded(|mut d| <[u8; 32]>::decode(&mut d))
				.map_err(|_| Error::<T>::BadAccountFormat)?;
			let qid = <crate::Pallet<T> as QueryHandler>::new_query(
				Junction::AccountId32 { network: None, id },
				100u32.into(),
				querier,
			);
			Self::deposit_event(Event::<T>::QueryPrepared(qid));
			Ok(())
		}

		#[pallet::call_index(1)]
		#[pallet::weight(Weight::from_parts(1_000_000, 1_000_000))]
		pub fn prepare_new_notify_query(
			origin: OriginFor<T>,
			querier: MultiLocation,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let id = who
				.using_encoded(|mut d| <[u8; 32]>::decode(&mut d))
				.map_err(|_| Error::<T>::BadAccountFormat)?;
			let call =
				Call::<T>::notification_received { query_id: 0, response: Default::default() };
			let qid = crate::Pallet::<T>::new_notify_query(
				Junction::AccountId32 { network: None, id },
				<T as Config>::RuntimeCall::from(call),
				100u32.into(),
				querier,
			);
			Self::deposit_event(Event::<T>::NotifyQueryPrepared(qid));
			Ok(())
		}

		#[pallet::call_index(2)]
		#[pallet::weight(Weight::from_parts(1_000_000, 1_000_000))]
		pub fn notification_received(
			origin: OriginFor<T>,
			query_id: QueryId,
			response: Response,
		) -> DispatchResult {
			let responder = ensure_response(<T as Config>::RuntimeOrigin::from(origin))?;
			Self::deposit_event(Event::<T>::ResponseReceived(responder, query_id, response));
			Ok(())
		}
	}
}

construct_runtime!(
	pub enum Test
	{
		System: frame_system::{Pallet, Call, Storage, Config<T>, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		ParasOrigin: origin::{Pallet, Origin},
		XcmPallet: pallet_xcm::{Pallet, Call, Storage, Event<T>, Origin, Config<T>},
		TestNotifier: pallet_test_notifier::{Pallet, Call, Event<T>},
	}
);

thread_local! {
	pub static SENT_XCM: RefCell<Vec<(MultiLocation, Xcm<()>)>> = RefCell::new(Vec::new());
}
pub(crate) fn sent_xcm() -> Vec<(MultiLocation, Xcm<()>)> {
	SENT_XCM.with(|q| (*q.borrow()).clone())
}
pub(crate) fn take_sent_xcm() -> Vec<(MultiLocation, Xcm<()>)> {
	SENT_XCM.with(|q| {
		let mut r = Vec::new();
		std::mem::swap(&mut r, &mut *q.borrow_mut());
		r
	})
}
/// Sender that never returns error.
pub struct TestSendXcm;
impl SendXcm for TestSendXcm {
	type Ticket = (MultiLocation, Xcm<()>);
	fn validate(
		dest: &mut Option<MultiLocation>,
		msg: &mut Option<Xcm<()>>,
	) -> SendResult<(MultiLocation, Xcm<()>)> {
		let pair = (dest.take().unwrap(), msg.take().unwrap());
		Ok((pair, MultiAssets::new()))
	}
	fn deliver(pair: (MultiLocation, Xcm<()>)) -> Result<XcmHash, SendError> {
		let hash = fake_message_hash(&pair.1);
		SENT_XCM.with(|q| q.borrow_mut().push(pair));
		Ok(hash)
	}
}
/// Sender that returns error if `X8` junction and stops routing
pub struct TestSendXcmErrX8;
impl SendXcm for TestSendXcmErrX8 {
	type Ticket = (MultiLocation, Xcm<()>);
	fn validate(
		dest: &mut Option<MultiLocation>,
		msg: &mut Option<Xcm<()>>,
	) -> SendResult<(MultiLocation, Xcm<()>)> {
		let (dest, msg) = (dest.take().unwrap(), msg.take().unwrap());
		if dest.len() == 8 {
			Err(SendError::Transport("Destination location full"))
		} else {
			Ok(((dest, msg), MultiAssets::new()))
		}
	}
	fn deliver(pair: (MultiLocation, Xcm<()>)) -> Result<XcmHash, SendError> {
		let hash = fake_message_hash(&pair.1);
		SENT_XCM.with(|q| q.borrow_mut().push(pair));
		Ok(hash)
	}
}

parameter_types! {
	pub Para3000: u32 = 3000;
	pub Para3000Location: MultiLocation = Parachain(Para3000::get()).into();
	pub Para3000PaymentAmount: u128 = 1;
	pub Para3000PaymentMultiAssets: MultiAssets = MultiAssets::from(MultiAsset::from((Here, Para3000PaymentAmount::get())));
}
/// Sender only sends to `Parachain(3000)` destination requiring payment.
pub struct TestPaidForPara3000SendXcm;
impl SendXcm for TestPaidForPara3000SendXcm {
	type Ticket = (MultiLocation, Xcm<()>);
	fn validate(
		dest: &mut Option<MultiLocation>,
		msg: &mut Option<Xcm<()>>,
	) -> SendResult<(MultiLocation, Xcm<()>)> {
		if let Some(dest) = dest.as_ref() {
			if !dest.eq(&Para3000Location::get()) {
				return Err(SendError::NotApplicable)
			}
		} else {
			return Err(SendError::NotApplicable)
		}

		let pair = (dest.take().unwrap(), msg.take().unwrap());
		Ok((pair, Para3000PaymentMultiAssets::get()))
	}
	fn deliver(pair: (MultiLocation, Xcm<()>)) -> Result<XcmHash, SendError> {
		let hash = fake_message_hash(&pair.1);
		SENT_XCM.with(|q| q.borrow_mut().push(pair));
		Ok(hash)
	}
}

parameter_types! {
	pub const BlockHashCount: u64 = 250;
}

impl frame_system::Config for Test {
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type Nonce = u64;
	type Hash = H256;
	type Hashing = ::sp_runtime::traits::BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Block = Block;
	type RuntimeEvent = RuntimeEvent;
	type BlockHashCount = BlockHashCount;
	type BlockWeights = ();
	type BlockLength = ();
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type DbWeight = ();
	type BaseCallFilter = Everything;
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub ExistentialDeposit: Balance = 1;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Test {
	type MaxLocks = MaxLocks;
	type Balance = Balance;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type FreezeIdentifier = ();
	type MaxHolds = ConstU32<0>;
	type MaxFreezes = ConstU32<0>;
}

parameter_types! {
	pub const RelayLocation: MultiLocation = Here.into_location();
	pub const AnyNetwork: Option<NetworkId> = None;
	pub UniversalLocation: InteriorMultiLocation = Here;
	pub UnitWeightCost: u64 = 1_000;
}

pub type SovereignAccountOf =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<AnyNetwork, AccountId>);

pub type LocalAssetTransactor =
	XcmCurrencyAdapter<Balances, IsConcrete<RelayLocation>, SovereignAccountOf, AccountId, ()>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<SovereignAccountOf, RuntimeOrigin>,
	ChildParachainAsNative<origin::Origin, RuntimeOrigin>,
	SignedAccountId32AsNative<AnyNetwork, RuntimeOrigin>,
	ChildSystemParachainAsSuperuser<ParaId, RuntimeOrigin>,
);

parameter_types! {
	pub const BaseXcmWeight: Weight = Weight::from_parts(1_000, 1_000);
	pub CurrencyPerSecondPerByte: (AssetId, u128, u128) = (Concrete(RelayLocation::get()), 1, 1);
	pub TrustedAssets: (MultiAssetFilter, MultiLocation) = (All.into(), Here.into());
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
	pub XcmFeesTargetAccount: AccountId = AccountId::new([167u8; 32]);
}

pub const XCM_FEES_NOT_WAIVED_USER_ACCOUNT: [u8; 32] = [37u8; 32];
match_types! {
	pub type XcmFeesNotWaivedLocations: impl Contains<MultiLocation> = {
		MultiLocation { parents: 0, interior: X1(Junction::AccountId32 {network: None, id: XCM_FEES_NOT_WAIVED_USER_ACCOUNT})}
	};
}

pub type Barrier = (
	TakeWeightCredit,
	AllowTopLevelPaidExecutionFrom<Everything>,
	AllowKnownQueryResponses<XcmPallet>,
	AllowSubscriptionsFrom<Everything>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = (TestPaidForPara3000SendXcm, TestSendXcm);
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	type IsReserve = ();
	type IsTeleporter = Case<TrustedAssets>;
	type UniversalLocation = UniversalLocation;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<BaseXcmWeight, RuntimeCall, MaxInstructions>;
	type Trader = FixedRateOfFungible<CurrencyPerSecondPerByte, ()>;
	type ResponseHandler = XcmPallet;
	type AssetTrap = XcmPallet;
	type AssetLocker = ();
	type AssetExchanger = ();
	type AssetClaims = XcmPallet;
	type SubscriptionService = XcmPallet;
	type PalletInstancesInfo = AllPalletsWithSystem;
	type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
	type FeeManager = XcmFeeManagerFromComponents<
		EverythingBut<XcmFeesNotWaivedLocations>,
		XcmFeeToAccount<Self::AssetTransactor, AccountId, XcmFeesTargetAccount>,
	>;
	type MessageExporter = ();
	type UniversalAliases = Nothing;
	type CallDispatcher = RuntimeCall;
	type SafeCallFilter = Everything;
	type Aliasers = Nothing;
}

pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, AnyNetwork>;

parameter_types! {
	pub static AdvertisedXcmVersion: pallet_xcm::XcmVersion = 3;
}

#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
	pub ReachableDest: Option<MultiLocation> = Some(Parachain(1000).into());
}

impl pallet_xcm::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmRouter = (TestSendXcmErrX8, TestPaidForPara3000SendXcm, TestSendXcm);
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmExecuteFilter = Everything;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type Weigher = FixedWeightBounds<BaseXcmWeight, RuntimeCall, MaxInstructions>;
	type UniversalLocation = UniversalLocation;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = AdvertisedXcmVersion;
	type TrustedLockers = ();
	type SovereignAccountOf = AccountId32Aliases<(), AccountId32>;
	type Currency = Balances;
	type CurrencyMatcher = IsConcrete<RelayLocation>;
	type MaxLockers = frame_support::traits::ConstU32<8>;
	type MaxRemoteLockConsumers = frame_support::traits::ConstU32<0>;
	type RemoteLockConsumerIdentifier = ();
	type WeightInfo = TestWeightInfo;
	#[cfg(feature = "runtime-benchmarks")]
	type ReachableDest = ReachableDest;
	type AdminOrigin = EnsureRoot<AccountId>;
}

impl origin::Config for Test {}

impl pallet_test_notifier::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
}

pub(crate) fn last_event() -> RuntimeEvent {
	System::events().pop().expect("RuntimeEvent expected").event
}

pub(crate) fn last_events(n: usize) -> Vec<RuntimeEvent> {
	System::events().into_iter().map(|e| e.event).rev().take(n).rev().collect()
}

pub(crate) fn buy_execution<C>(fees: impl Into<MultiAsset>) -> Instruction<C> {
	use xcm::latest::prelude::*;
	BuyExecution { fees: fees.into(), weight_limit: Unlimited }
}

pub(crate) fn buy_limited_execution<C>(
	fees: impl Into<MultiAsset>,
	weight: Weight,
) -> Instruction<C> {
	use xcm::latest::prelude::*;
	BuyExecution { fees: fees.into(), weight_limit: Limited(weight) }
}

pub(crate) fn new_test_ext_with_balances(
	balances: Vec<(AccountId, Balance)>,
) -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();

	pallet_balances::GenesisConfig::<Test> { balances }
		.assimilate_storage(&mut t)
		.unwrap();

	pallet_xcm::GenesisConfig::<Test> { safe_xcm_version: Some(2), ..Default::default() }
		.assimilate_storage(&mut t)
		.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

pub(crate) fn fake_message_hash<T>(message: &Xcm<T>) -> XcmHash {
	message.using_encoded(sp_io::hashing::blake2_256)
}
