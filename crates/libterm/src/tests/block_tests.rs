use uuid::Uuid;

use crate::block::detector::BlockDetector;
use crate::block::store::{BlockStatus, BlockStore};
use crate::vt::sequences::Osc133;

#[test]
fn block_lifecycle_success() {
    let session_id = Uuid::new_v4();
    let mut store = BlockStore::default();
    let mut detector = BlockDetector::new(session_id);

    detector.start_block("echo hello", &mut store);
    assert_eq!(store.all().len(), 1);
    assert_eq!(store.all()[0].status, BlockStatus::Running);

    detector.finish_block(0, b"hello\n".to_vec(), &mut store);
    assert_eq!(store.all()[0].status, BlockStatus::Success);
    assert_eq!(store.all()[0].exit_code, Some(0));
}

#[test]
fn block_lifecycle_error() {
    let session_id = Uuid::new_v4();
    let mut store = BlockStore::default();
    let mut detector = BlockDetector::new(session_id);

    detector.start_block("cargo test", &mut store);
    detector.finish_block(1, b"FAILED".to_vec(), &mut store);
    assert_eq!(store.all()[0].status, BlockStatus::Error);
    assert_eq!(store.all()[0].exit_code, Some(1));
}

#[test]
fn multiple_blocks_independent() {
    let session_id = Uuid::new_v4();
    let mut store = BlockStore::default();
    let mut detector = BlockDetector::new(session_id);

    detector.start_block("cmd1", &mut store);
    detector.finish_block(0, vec![], &mut store);
    detector.start_block("cmd2", &mut store);
    detector.finish_block(1, vec![], &mut store);

    assert_eq!(store.all().len(), 2);
    assert_eq!(store.all()[0].status, BlockStatus::Success);
    assert_eq!(store.all()[1].status, BlockStatus::Error);
}

#[test]
fn osc133_parse() {
    assert_eq!(Osc133::parse("133;A"), Some(Osc133::PromptStart));
    assert_eq!(Osc133::parse("133;C"), Some(Osc133::CommandStart));
    assert_eq!(Osc133::parse("133;D;0"), Some(Osc133::CommandFinished(0)));
    assert_eq!(
        Osc133::parse("133;D;127"),
        Some(Osc133::CommandFinished(127))
    );
    assert_eq!(Osc133::parse("999;X"), None);
}
