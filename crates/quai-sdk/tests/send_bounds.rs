//! A desktop wallet runs these on a multi-threaded runtime, so their futures
//! must be Send. Compile-time only: nothing here runs.
//!
//! `SqliteStore` holds a `!Sync` connection, so a session helper that awaits
//! while borrowing `&self` makes every future that calls it `!Send`. Sessions
//! therefore take `&mut self` throughout; this file keeps it that way.
#![cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
#![allow(dead_code)]
use quai_sdk::account_preflight::{AccountIntent, FeePolicy};
use quai_sdk::accounts::AccountSession;
use quai_sdk::qi::{QiChangePool, QiSession};
use quai_sdk::qi_preflight::{QiIntent, QiPolicy};
use quai_sdk::signer::LocalSigner;
use quai_sdk::wallet::discovery::{DiscoveryRequest, NetworkScope, discover};
use quai_sdk::wallet::storage::{ReservationId, SqliteStore};
use quai_sdk::wallet::{AccountPublic, HdWallet};
use quai_sdk::{BlockTag, DynTransport, Provider, QuaiAddress};

type P = Provider<DynTransport>;
fn send<F: Send>(_: F) {}

fn qi_session(
    p: &P,
    wallet: &HdWallet,
    store: &mut SqliteStore,
    id: ReservationId,
    intent: QiIntent,
    policy: QiPolicy,
    change: QiChangePool,
) {
    let mut session = QiSession::new(p, wallet, store).unwrap();
    send(session.prepare(id, intent, policy, change));
    send(session.broadcast(id));
}

fn account_session(
    p: &P,
    signer: &LocalSigner,
    store: &mut SqliteStore,
    id: ReservationId,
    intent: AccountIntent,
    policy: FeePolicy,
) {
    let mut session = AccountSession::new(p, signer, store).unwrap();
    send(session.prepare(id, intent, policy));
    send(session.broadcast(id));
}

fn scans(
    p: &P,
    store: &mut SqliteStore,
    account: &AccountPublic,
    scope: NetworkScope,
    request: &DiscoveryRequest,
    accounts: &[QuaiAddress],
) {
    send(quai_sdk::qi_discovery::scan_qi(
        p,
        scope,
        account,
        &Default::default(),
        || false,
    ));
    send(quai_sdk::discovery::discover_qi(
        p,
        scope,
        account,
        &Default::default(),
        || false,
    ));
    let source = quai_sdk::discovery::AccountRpcSource::new(p);
    send(discover(&source, account, request, || false));
    send(p.account_states(accounts, BlockTag::Latest));
    send(quai_sdk::qi_discovery::refresh_qi(p, store, 10, || false));
    send(quai_sdk::qi_discovery::refresh_qi_with(
        p,
        store,
        Default::default(),
        || false,
    ));
}
