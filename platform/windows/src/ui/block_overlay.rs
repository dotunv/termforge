//! Block hit-test targets.
//!
//! Command blocks are first-class structured data, surfaced in the sidebar's
//! RECENT BLOCKS list and the tab/pane status indicators.  We intentionally no
//! longer draw them as floating overlay cards on top of a pane — the live
//! terminal grid already renders each command and its output, so an overlay
//! only duplicated that content inside a border.  This module keeps the spatial
//! hit-target type so click-to-copy can be reintroduced later (e.g. via subtle
//! inline command dividers) without reshaping the call sites.

use libterm::block::store::BlockId;
use rstar::{RTree, RTreeObject, AABB};

#[derive(Debug, Clone)]
pub struct BlockHitTarget {
    pub block_id: BlockId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RTreeObject for BlockHitTarget {
    type Envelope = AABB<[f32; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.x, self.y], [self.x + self.w, self.y + self.h])
    }
}

pub fn build_block_rtree(targets: Vec<BlockHitTarget>) -> RTree<BlockHitTarget> {
    RTree::bulk_load(targets)
}
