// SPDX-License-Identifier: MIT OR Apache-2.0

// Copyright 2025 Eadf (github.com/eadf)
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::linkedlist::IsLessThan;
use crate::linkedlist::LinkedList;
use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct RoundedTen(pub(crate) i32);

impl IsLessThan for RoundedTen {
    fn is_less_than(&self, other: &Self) -> bool {
        self < other
    }
}

impl Ord for RoundedTen {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0 / 10).cmp(&(other.0 / 10))
    }
}

impl PartialOrd for RoundedTen {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for RoundedTen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[allow(dead_code)]
pub(super) fn test_free_pool_uniqueness<K, V>(list: &LinkedList<K, V>)
where
    K: Debug + Ord + Clone + IsLessThan,
    V: Debug + Clone,
{
    let mut l2: Vec<usize> = list.free_index_pool.iter().copied().collect();
    l2.sort();
    l2.dedup();
    assert_eq!(
        list.free_index_pool.len(),
        l2.len(),
        "free_index_pool_ contains duplicate elements"
    );
    // make sure every item in free pool points to a None
    assert!(
        list.free_index_pool
            .iter()
            .all(|&i| list.nodes[i].kv.is_none())
    );
}

#[allow(dead_code)]
pub(super) fn test_head_and_tail_is_some<K, V>(list: &LinkedList<K, V>)
where
    K: Debug + Ord + Clone + IsLessThan,
    V: Debug + Clone,
{
    assert!(list.first().is_some());
    assert!(list.last().is_some());
    assert!(list.is_pos_valid(list.first()));
    assert!(list.is_pos_valid(list.last()));
}

#[allow(dead_code)]
pub(super) fn test_head_and_tail_is_none<K, V>(list: &LinkedList<K, V>)
where
    K: Debug + Ord + Clone + IsLessThan,
    V: Debug + Clone,
{
    assert!(list.first().is_none());
    assert!(list.last().is_none());
}
