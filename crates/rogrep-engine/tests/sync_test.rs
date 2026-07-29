//! End-to-end sync pipeline tests against a fake home directory.

use rogrep_engine::{sync, NoopIndexer, SyncOptions};
use rogrep_model::config::Config;
use rogrep_model::paths::DataLayout;
use rogrep_store::Store;
use std::fs;
use std::path::Path;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../rogrep-parsers/fixtures/claude/basic_session.jsonl"
);

struct Env {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
    layout: DataLayout,
    session: std::path::PathBuf,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = home.join(".claude/projects/-home-u-src-proj");
    fs::create_dir_all(&proj).unwrap();
    let session = proj.join("sess-1.jsonl");
    fs::copy(FIXTURE, &session).unwrap();
    let layout = DataLayout::new(tmp.path().join("data"));
    Env {
        _tmp: tmp,
        home,
        layout,
        session,
    }
}

fn run_sync(env: &Env, store: &mut Store) -> rogrep_engine::SyncReport {
    let config = Config::default();
    let options = SyncOptions {
        full: false,
        home: env.home.clone(),
    };
    sync(&env.layout, &config, store, &mut NoopIndexer, &options, &mut |_| {}).unwrap()
}

fn bump_mtime(path: &Path) {
    // Ensure mtime changes even on coarse filesystems.
    let file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
        .unwrap();
}

#[test]
fn sync_ingests_then_noops() {
    let env = setup();
    let mut store = Store::open(&env.layout.db_path()).unwrap();

    let r1 = run_sync(&env, &mut store);
    assert!(r1.synced);
    assert_eq!(r1.files_changed, 1);
    assert_eq!(r1.turns_written, 7, "all fixture turns written");
    assert!(r1.errors.is_empty());

    let convs = store.recent_conversations(10, None).unwrap();
    assert_eq!(convs.len(), 1);
    let conv = &convs[0];
    assert_eq!(conv.turn_count, 7);
    assert_eq!(conv.exchange_count, 2);
    assert_eq!(conv.title.as_deref(), Some("Fix flaky reader offset test"));
    assert_eq!(conv.normalized_project, "-home-u-src-proj");

    // Second sync: nothing changed, nothing parsed.
    let r2 = run_sync(&env, &mut store);
    assert!(r2.synced);
    assert_eq!(r2.files_changed, 0);
    assert_eq!(r2.turns_written, 0);
}

#[test]
fn append_parses_only_the_tail() {
    let env = setup();
    let mut store = Store::open(&env.layout.db_path()).unwrap();
    run_sync(&env, &mut store);

    // Append a new exchange.
    let extra = concat!(
        r#"{"type":"user","message":{"role":"user","content":"one more thing"},"uuid":"u9","timestamp":"2026-07-01T11:00:00.000Z","sessionId":"sess-1"}"#,
        "\n",
        r#"{"type":"assistant","message":{"model":"claude-fable-5","id":"msg_09","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":1,"output_tokens":2}},"uuid":"a9","timestamp":"2026-07-01T11:00:05.000Z","sessionId":"sess-1"}"#,
        "\n"
    );
    let mut content = fs::read(&env.session).unwrap();
    content.extend_from_slice(extra.as_bytes());
    fs::write(&env.session, &content).unwrap();
    bump_mtime(&env.session);

    let r = run_sync(&env, &mut store);
    assert_eq!(r.files_changed, 1);
    // Tail = the open exchange ("looks good, ship it" + reply) + the two new
    // turns — NOT the whole 10-turn file.
    assert_eq!(r.turns_written, 4);

    let conv = &store.recent_conversations(10, None).unwrap()[0];
    assert_eq!(conv.turn_count, 9);
    assert_eq!(conv.exchange_count, 3);

    let exchanges = store.exchanges_for(&conv.id).unwrap();
    assert_eq!(exchanges.len(), 3);
    assert_eq!(exchanges[2].user_preview, "one more thing");
    // Ordinals are dense and ranges contiguous.
    for (i, e) in exchanges.iter().enumerate() {
        assert_eq!(e.ordinal as usize, i);
    }
    for w in exchanges.windows(2) {
        assert_eq!(w[0].end_turn, w[1].start_turn);
    }
}

#[test]
fn rewrite_triggers_full_reparse() {
    let env = setup();
    let mut store = Store::open(&env.layout.db_path()).unwrap();
    run_sync(&env, &mut store);

    // Rewrite the file with different early content (simulates spool
    // rewrite / truncation).
    let content = fs::read_to_string(&env.session).unwrap();
    let rewritten = content.replace("Fix the flaky parser test", "Totally different opening");
    fs::write(&env.session, rewritten).unwrap();
    bump_mtime(&env.session);

    let r = run_sync(&env, &mut store);
    assert_eq!(r.files_changed, 1);
    assert_eq!(r.turns_written, 7, "fingerprint mismatch → full reparse");
}

#[test]
fn deleted_file_removes_conversation() {
    let env = setup();
    let mut store = Store::open(&env.layout.db_path()).unwrap();
    run_sync(&env, &mut store);
    fs::remove_file(&env.session).unwrap();

    let r = run_sync(&env, &mut store);
    assert_eq!(r.files_removed, 1);
    assert!(store.recent_conversations(10, None).unwrap().is_empty());
}

#[test]
fn usage_stats_bucket_daily() {
    let env = setup();
    let mut store = Store::open(&env.layout.db_path()).unwrap();
    run_sync(&env, &mut store);

    let tz = jiff::tz::TimeZone::UTC;
    let usage = rogrep_store::stats::usage_report(&store, rogrep_store::stats::Period::Daily, &tz, None, None).unwrap();
    assert_eq!(usage.len(), 1);
    let day = usage.get("2026-07-01").expect("fixture day present");
    assert_eq!(day.turns, 7);
    assert_eq!(day.conversations, 1);
    assert_eq!(day.exchanges, 2);
    assert_eq!(day.output_tokens, 92);
}
