// SPDX-License-Identifier: MIT OR Apache-2.0

// Copyright 2025 Eadf (github.com/eadf)
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::prelude::CppMapError;
use crate::prelude::SkipList;
use crate::skiplist::{HEAD_INDEX, IsLessThan};
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
fn test_free_pool_uniqueness<K, V>(list: &SkipList<K, V>)
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
pub fn test_head_and_tail_is_some<K, V>(list: &SkipList<K, V>)
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
pub fn test_head_and_tail_is_none<K, V>(list: &SkipList<K, V>)
where
    K: Debug + Ord + Clone + IsLessThan,
    V: Debug + Clone,
{
    assert!(list.first().is_none());
    assert!(list.last().is_none());
}

#[test]
fn test_remove() {
    let mut list = SkipList::default();
    let _ = list.insert(1, "one");
    let _ = list.insert(2, "two");
    let _ = list.insert(3, "three");

    assert_eq!(list.remove(&2).unwrap().1, "two");
    test_free_pool_uniqueness(&list);
    list.debug_print();
    assert_eq!(list.get(&2), None);
    assert_eq!(list.remove(&2), None); // Double remove
    test_free_pool_uniqueness(&list);
    test_head_and_tail_is_some(&list);
}

#[test]
fn test_free_pool_reuse() -> Result<(), CppMapError> {
    let mut list = SkipList::default();
    let _ = list.insert(1, "one")?;
    let _ = list.insert(2, "two")?;

    let initial_len = list.nodes.len();

    let _ = list.remove(&1);
    test_free_pool_uniqueness(&list);
    test_head_and_tail_is_some(&list);
    let _ = list.remove(&2);
    test_free_pool_uniqueness(&list);

    // After removal, indices should go to free pool
    assert!(!list.free_index_pool.is_empty());

    let _ = list.insert(3, "three")?;

    // Should reuse from free pool
    assert!(list.nodes.len() <= initial_len);
    test_head_and_tail_is_some(&list);
    Ok(())
}

#[test]
fn test_remove_head() {
    let mut list = SkipList::default();
    let _ = list.insert(1, "one");
    let _ = list.insert(2, "two");

    assert_eq!(list.remove(&1), Some((1, "one")));
    test_free_pool_uniqueness(&list);
    assert_eq!(list.get(&1), None);
    assert_eq!(list.get(&2), Some(&"two"));
    test_head_and_tail_is_some(&list);
}

#[test]
fn test_remove_tail() {
    let mut list = SkipList::default();
    let _ = list.insert(1, "one");
    let _ = list.insert(2, "two");

    assert_eq!(list.remove(&2), Some((2, "two")));
    test_free_pool_uniqueness(&list);
    test_head_and_tail_is_some(&list);
    assert_eq!(list.get(&2), None);
    assert_eq!(list.get(&1), Some(&"one"));
}

#[test]
fn test_get_by_index() -> Result<(), CppMapError> {
    let mut list = SkipList::default();
    let index1 = list.insert(1, "one")?;
    let _ = list.insert(2, "two")?;

    let index = list.first();
    if index.is_some() {
        assert_eq!(list.get_at(index1), Some((&1, &"one")));
    } else {
        panic!("Index not found");
    }
    Ok(())
}

#[test]
fn test_multiple_operations() -> Result<(), CppMapError> {
    let mut list = SkipList::default();
    let _ = list.insert(1, "one")?;
    let _ = list.insert(2, "two")?;
    let _ = list.insert(3, "three")?;

    assert_eq!(list.remove(&2), Some((2, "two")));
    assert_eq!(list.get(&2), None);

    let _ = list.insert(4, "four")?;
    assert_eq!(list.get(&4), Some(&"four"));

    assert_eq!(list.remove(&1), Some((1, "one")));
    test_free_pool_uniqueness(&list);
    assert_eq!(list.remove(&3), Some((3, "three")));
    test_free_pool_uniqueness(&list);
    test_head_and_tail_is_some(&list);
    assert_eq!(list.remove(&4), Some((4, "four")));
    test_free_pool_uniqueness(&list);

    // List should be empty now
    test_head_and_tail_is_none(&list);

    let _ = list.insert(3, "three")?;
    let _ = list.insert(1, "one")?;
    let _ = list.insert(0, "zero")?;
    Ok(())
}

#[test]
fn test_remove_current_maintains_invariants() {
    let mut list = SkipList::default();
    let _ = list.insert(1, 1.0);
    let _ = list.insert(2, 2.0);
    let _ = list.insert(3, 3.0);

    let mut cursor = list.lower_bound(&2);
    assert_eq!(list.get_v_at(cursor.unwrap()), Some(2.0).as_ref());

    let removed = list.remove_by_index(&mut cursor);
    assert_eq!(removed, Some((2, 2.0)));

    // Verify free pool and list integrity
    test_free_pool_uniqueness(&list);
    test_head_and_tail_is_some(&list);
    list.iter().for_each(|node| print!("{:?} ", node.1));
    assert_eq!(list.len(), 2);

    // Verify remaining elements can be accessed
    let cursor = list.lower_bound(&1);
    let cursor = list.next_pos(cursor).unwrap();
    assert_eq!(list.get_k_at(cursor), Some(&3));

    let cursor = list.lower_bound(&3);

    let cursor = list.prev_pos(cursor).unwrap();
    assert_eq!(list.get_v_at(cursor), Some(&1.0));
}

#[test]
fn test_change_key_of_node() -> Result<(), CppMapError> {
    let mut list = SkipList::default();
    let _ = list.insert(RoundedTen(10), 10.0)?;
    let _ = list.insert(RoundedTen(20), 20.0)?;

    let mut cursor = list.first();
    assert!(list.is_pos_valid(cursor));
    assert_eq!(list.get_k_at(cursor.unwrap()), Some(&RoundedTen(10)));
    let _ = list.change_key_of_node(cursor.unwrap(), RoundedTen(11))?;
    assert_eq!(list.get_k_at(cursor.unwrap()), Some(&RoundedTen(11)));
    cursor = list.next_pos(cursor);
    assert!(list.is_pos_valid(cursor));
    assert_eq!(list.get_k_at(cursor.unwrap()), Some(&RoundedTen(20)));
    let _ = list.change_key_of_node(cursor.unwrap(), RoundedTen(21))?;
    assert_eq!(list.get_k_at(cursor.unwrap()), Some(&RoundedTen(21)));
    cursor = list.next_pos(cursor);
    /*assert!(
        list.change_key_of_node(cursor.unwrap(), RoundedTen(21))
            .is_err()
    );*/
    assert!(!list.is_pos_valid(cursor));
    list.clear();
    Ok(())
}

#[test]
#[allow(clippy::just_underscores_and_digits)]
fn test_sequential_find_position_1() -> Result<(), CppMapError> {
    // loop to account for the random properties of a skip list

    for _i in 0..100 {
        let mut list = SkipList::default();
        let _50 = list.insert(RoundedTen(50), 50.0)?;
        list.validate();
        let _10 = list.insert(RoundedTen(10), 10.0)?;
        list.validate();
        let _30 = list.insert(RoundedTen(30), 30.0)?;
        list.validate();
        let _40 = list.insert(RoundedTen(40), 40.0)?;
        list.validate();
        let _20 = list.insert(RoundedTen(20), 20.0)?;
        list.validate();
        let indices = [_10, _20, _30, _40, _50];

        for hint in indices {
            assert_eq!(
                list.sequential_find_position_(&RoundedTen(0), hint.0),
                Some(HEAD_INDEX),
                "failed when searching from {hint:?}",
            );
        }
        for hint in indices {
            assert_eq!(
                list.sequential_find_position_(&RoundedTen(20), hint.0),
                Some(_10.0)
            );
        }
        for hint in indices {
            assert_eq!(
                list.sequential_find_position_(&RoundedTen(30), hint.0),
                Some(_20.0)
            );
        }
        for hint in indices {
            assert_eq!(
                list.sequential_find_position_(&RoundedTen(40), hint.0),
                Some(_30.0)
            );
        }
        for index in indices {
            assert_eq!(
                list.sequential_find_position_(&RoundedTen(70), index.0),
                Some(_50.0)
            );
        }
        for hint in indices {
            assert_eq!(
                list.sequential_find_position_(&RoundedTen(60), hint.0),
                Some(_50.0)
            );
        }
    }
    Ok(())
}

#[test]
#[allow(clippy::just_underscores_and_digits)]
fn test_sequential_find_position_2() -> Result<(), CppMapError> {
    // loop to account for the random properties of a skip list
    for _i in 0..100 {
        let mut list = SkipList::default();
        let _0 = list.insert(0, 0.0)?;
        list.debug_print();
        list.validate();
        let mut _1 = Some(list.insert(1, 1.0)?);
        list.validate();
        let _2 = list.insert(2, 2.0)?;
        list.validate();
        let _ = list.remove_by_index(&mut _1);
        list.validate();
        let indices = [_0, _2];

        for hint in indices {
            assert_eq!(list.sequential_find_position_(&2, hint.0), _1.map(|x| x.0));
        }
    }
    Ok(())
}
