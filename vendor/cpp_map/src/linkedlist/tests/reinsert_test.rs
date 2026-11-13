// SPDX-License-Identifier: MIT OR Apache-2.0

// Copyright 2025 Eadf (github.com/eadf)
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use super::common::RoundedTen;
use crate::CppMapError;
use crate::prelude::*;

#[test]
fn test_reinsert_node_comprehensive() -> Result<(), CppMapError> {
    for _ in 0..100 {
        // Basic test with a single node
        let mut list = SkipList::default();
        let idx_1 = list.insert(RoundedTen(1), "one")?;
        assert_eq!(list.get(&RoundedTen(1)), Some(&"one"));

        // Rekey the only node (should not change position but update key)
        let result = list.change_key_of_node(idx_1, RoundedTen(11));
        assert!(result.is_ok());
        let new_index = result?;
        //println!("{:?}", list);
        assert_eq!(new_index, idx_1);
        assert_eq!(list.get(&RoundedTen(2)), None);
        assert_eq!(list.get(&RoundedTen(3)), None);
        assert_eq!(list.get(&RoundedTen(1)), None);
        assert_eq!(list.get(&RoundedTen(10)), Some(&"one"));
        assert_eq!(list.get_v_at(idx_1), Some(&"one"));

        // Test with multiple nodes
        let mut list = SkipList::default();
        let idx_1 = list.insert(RoundedTen(1), "zero")?;
        let _idx_3 = list.insert(RoundedTen(13), "ten")?;
        let _idx_5 = list.insert(RoundedTen(50), "fifty")?;
        let _idx_7 = list.insert(RoundedTen(70), "seventy")?;
        let idx_9 = list.insert(RoundedTen(90), "ninty")?;

        // Rekey the first node (1 -> 0)
        let result = list.change_key_of_node(idx_1, RoundedTen(0));

        assert!(result.is_ok());
        assert_eq!(list.get(&RoundedTen(0)), Some(&"zero"));
        assert_eq!(list.get(&RoundedTen(1)), Some(&"zero"));
        assert_eq!(list.get_v_at(idx_1), Some(&"zero"));

        // Verify node order (forward traversal)
        let mut cursor = list.first();
        let expected_keys: Vec<_> = [0, 13, 50, 70, 90].into_iter().map(RoundedTen).collect();
        let mut key_idx = 0;

        while let Some(_idx) = cursor {
            let node = list.get_at(cursor.unwrap()).unwrap();
            assert_eq!(*node.0, expected_keys[key_idx]);
            key_idx += 1;
            cursor = list.next_pos(cursor);
        }

        // Rekey the last node (9 -> 10)
        let result = list.change_key_of_node(idx_9, RoundedTen(91));

        assert!(result.is_ok());
        assert_eq!(list.get(&RoundedTen(100)), None);
        assert_eq!(list.get(&RoundedTen(91)), Some(&"ninty"));
        assert_eq!(list.get_v_at(idx_9), Some(&"ninty"));

        // Verify node order after last node rekey
        let mut cursor = list.first();
        let expected_keys: Vec<_> = [0, 13, 50, 70, 91].into_iter().map(RoundedTen).collect();
        let mut key_idx = 0;

        while cursor.is_some() {
            let node = list.get_at(cursor.unwrap()).unwrap();

            assert_eq!(node.0.to_string(), expected_keys[key_idx].to_string());
            key_idx += 1;
            cursor = list.next_pos(cursor);
        }
    }
    Ok(())
}

#[test]
fn test_reinsert_node_sequential() -> Result<(), CppMapError> {
    // Create a list with sequential keys and verify that reinsert_node
    // correctly maintains order during various operations
    let mut list = SkipList::default();

    // Insert sequential nodes
    let idx_1 = list.insert(RoundedTen(10), "Ten")?;
    let idx_2 = list.insert(RoundedTen(20), "Twenty")?;
    let _idx_3 = list.insert(RoundedTen(30), "Thirty")?;
    let _idx_4 = list.insert(RoundedTen(40), "Forty")?;
    let idx_5 = list.insert(RoundedTen(51), "Fifty")?;

    // Verify initial state
    assert_eq!(list.first(), Some(idx_1));
    assert_eq!(list.last(), Some(idx_5));

    // Test various reinsert operations

    // 1. Move a node forward in the list (20 -> 21)
    let _ = list.change_key_of_node(idx_2, RoundedTen(21))?;

    // 2. Move a node backward in the list (51 -> 50)
    let _ = list.change_key_of_node(idx_5, RoundedTen(50))?;

    // 3. Move the head toward the middle (10 -> 11)
    let _ = list.change_key_of_node(idx_1, RoundedTen(11))?;

    // Verify final order is 15, 25, 30, 35, 40
    let expected_keys: Vec<_> = [11, 21, 30, 40, 50].into_iter().map(RoundedTen).collect();
    let expected_values = ["Ten", "Twenty", "Thirty", "Forty", "Fifty"];

    let mut cursor = list.first();
    let mut key_idx = 0;

    while cursor.is_some() {
        let node = list.get_at(cursor.unwrap());
        assert_eq!(node.unwrap().0, &expected_keys[key_idx]);
        assert_eq!(node.unwrap().1, &expected_values[key_idx]);
        key_idx += 1;
        cursor = list.next_pos(cursor);
    }

    // Verify the prev/next pointers are correct by reverse traversal
    let mut cursor = list.last();
    let mut key_idx = 4;

    while cursor.is_some() {
        let node = list.get_at(cursor.unwrap());
        assert_eq!(node.unwrap().0.0, expected_keys[key_idx].0);
        assert_eq!(node.unwrap().1, &expected_values[key_idx]);

        cursor = list.prev_pos(cursor);
        if key_idx > 0 {
            key_idx -= 1;
        } else {
            break;
        }
    }
    Ok(())
}

#[test]
fn test_reinsert_node_with_delete_and_reinsert() -> Result<(), CppMapError> {
    // Test interaction between delete, insert, and reinsert operations
    let mut list = SkipList::default();

    // Insert initial nodes
    let _idx_10 = list.insert(RoundedTen(10), "Ten");
    let _idx_20 = list.insert(RoundedTen(20), "Twenty");
    let _idx_30 = list.insert(RoundedTen(30), "Thirty");

    // Delete the middle node
    let _ = list.remove(&RoundedTen(20));

    // Insert a new node that will reuse the freed index
    let idx_20 = list.insert(RoundedTen(25), "Twenty-five")?;

    // Check if the new node can be reinserted
    let _ = list.change_key_of_node(idx_20, RoundedTen(15))?;
    assert_eq!(list.get(&RoundedTen(25)), None);
    assert_eq!(list.get(&RoundedTen(15)), Some(&"Ten"));

    // Verify order: 10, 15, 30
    let mut cursor = list.first();
    let expected_keys: Vec<_> = [10, 15, 30].into_iter().map(RoundedTen).collect();
    let expected_values = ["Ten", "Twenty-five", "Thirty"];
    let mut key_idx = 0;

    while cursor.is_some() {
        let node = list.get_at(cursor.unwrap());
        assert_eq!(node.unwrap().0.0, expected_keys[key_idx].0);
        assert_eq!(node.unwrap().1, &expected_values[key_idx]);
        key_idx += 1;
        cursor = list.next_pos(cursor);
    }
    Ok(())
}

#[ignore]
#[test]
fn test_key_change_with_ordering_conflict() -> Result<(), CppMapError> {
    for _i in 0..100 {
        // Test how key changes are handled when the new key would normally change the ordering
        let mut list = SkipList::default();

        // Insert nodes with some space between them
        let _idx_10 = list.insert(RoundedTen(10), "Ten")?;
        let idx_20 = list.insert(RoundedTen(20), "Twenty")?;
        let idx_30 = list.insert(RoundedTen(30), "Thirty")?;
        let idx_40 = list.insert(RoundedTen(40), "Forty")?;

        // Now change key of the "20" node to "35" which would normally place it after "30"
        // But the rank should keep it in its original position
        let _ = list.change_key_of_node(idx_20, RoundedTen(35))?;

        // Verify the key was changed
        assert_eq!(list.get(&RoundedTen(20)), None);
        assert_eq!(list.get(&RoundedTen(35)), Some(&"Twenty"));
        assert_eq!(list.get(&RoundedTen(30)), Some(&"Twenty"));

        // Verify order is maintained by rank: 10, 35(was 20), 30, 40
        // This is the critical test - order should still be based on insertion order (rank)
        // rather than the new key values
        let mut cursor = list.first();
        let expected_keys = [
            RoundedTen(10),
            RoundedTen(35),
            RoundedTen(30),
            RoundedTen(40),
        ];
        let expected_values = ["Ten", "Twenty", "Thirty", "Forty"];
        let mut key_idx = 0;

        while cursor.is_some() {
            let node = list.get_at(cursor.unwrap());
            assert_eq!(node.unwrap().0, &expected_keys[key_idx]);
            assert_eq!(node.unwrap().1, &expected_values[key_idx]);
            key_idx += 1;
            cursor = list.next_pos(cursor);
        }

        // Also test changing a key to move it backward in the ordering
        // Change "30" to "16", which would normally place it between "15" and "35"
        let _ = list.change_key_of_node(idx_30, RoundedTen(16))?;

        // Verify the key was changed
        list.debug_print();
        assert_eq!(list.get(&RoundedTen(30)), Some(&"Twenty"));

        // Verify order is still maintained: 10, 35(was 20), 15(was 30), 40
        let mut cursor = list.first();
        let expected_keys = [
            RoundedTen(10),
            RoundedTen(35),
            RoundedTen(16),
            RoundedTen(40),
        ];
        let expected_values = ["Ten", "Twenty", "Thirty", "Forty"];
        let mut key_idx = 0;

        while cursor.is_some() {
            let node = list.get_at(cursor.unwrap());
            assert_eq!(node.unwrap().0, &expected_keys[key_idx]);
            assert_eq!(node.unwrap().1, &expected_values[key_idx]);
            key_idx += 1;
            cursor = list.next_pos(cursor);
        }

        // Test another edge case: move a key to a value that would cause it to "skip" multiple positions
        // Change "40" to "5", which would normally place it at the beginning
        let _ = list.change_key_of_node(idx_40, RoundedTen(5))?;

        // Verify the key change
        assert_eq!(list.get(&RoundedTen(40)), None);
        #[cfg(feature = "console_debug")]
        list.debug_print();

        // The order should still be: 10, 35(was 20), 15(was 30), 5(was 40)
        let mut cursor = list.first();
        let expected_keys = [
            RoundedTen(10),
            RoundedTen(35),
            RoundedTen(15),
            RoundedTen(5),
        ];
        let expected_values = ["Ten", "Twenty", "Thirty", "Forty"];
        let mut key_idx = 0;

        while cursor.is_some() {
            let node = list.get_at(cursor.unwrap());
            assert_eq!(node.unwrap().0, &expected_keys[key_idx]);
            assert_eq!(node.unwrap().1, &expected_values[key_idx]);
            key_idx += 1;
            cursor = list.next_pos(cursor);
        }
    }

    Ok(())
}

#[test]
fn test_key_change_with_ordering_conflict_2() -> Result<(), CppMapError> {
    for _i in 0..100 {
        // Test how key changes are handled when the new key would normally change the ordering
        let mut list = SkipList::default();

        // Insert nodes with some space between them
        let _idx_10 = list.insert(RoundedTen(10), "Ten")?;
        let idx_20 = list.insert(RoundedTen(20), "Twenty")?;
        let _idx_30 = list.insert(RoundedTen(30), "Thirty")?;

        // Now change key of the "20" node to "35" which would normally place it after "30"
        // But the rank should keep it in its original position
        let _ = list.change_key_of_node(idx_20, RoundedTen(34))?;
        let _idx_40 = list.insert(RoundedTen(40), "Forty")?;
        let _idx_35 = list.insert(RoundedTen(35), "ThirtyFive")?; // nop

        // Verify the key was changed
        assert_eq!(list.get(&RoundedTen(20)), None);
        assert_eq!(list.get(&RoundedTen(34)), Some(&"Twenty"));
        assert_eq!(list.get(&RoundedTen(30)), Some(&"Twenty"));
        assert_eq!(list.get(&RoundedTen(41)), Some(&"Forty"));

        println!("{list:?}");
        let mut cursor = list.first();
        let expected_keys = [
            RoundedTen(10),
            RoundedTen(34),
            RoundedTen(30),
            RoundedTen(40),
        ];
        let expected_values = ["Ten", "Twenty", "Thirty", "Forty"];
        let mut key_idx = 0;

        while cursor.is_some() {
            let node = list.get_at(cursor.unwrap());
            assert_eq!(node.unwrap().0, &expected_keys[key_idx]);
            assert_eq!(node.unwrap().1, &expected_values[key_idx]);
            key_idx += 1;
            cursor = list.next_pos(cursor);
        }
    }

    Ok(())
}

#[test]
fn test_key_change_with_ordering_conflict_3() -> Result<(), CppMapError> {
    for _i in 0..100 {
        // Create a list with sequential keys and verify that reinsert_node
        // correctly maintains order during various operations
        let mut list = SkipList::default();

        // Insert sequential nodes
        let idx_1 = list.insert(RoundedTen(10), "Ten")?;
        let idx_2 = list.insert(RoundedTen(20), "Twenty")?;
        let _idx_3 = list.insert(RoundedTen(30), "Thirty")?;
        let _idx_4 = list.insert(RoundedTen(40), "Forty")?;
        let idx_5 = list.insert(RoundedTen(51), "Fifty")?;

        // Verify initial state
        assert_eq!(list.first(), Some(idx_1));
        assert_eq!(list.last(), Some(idx_5));

        // Test various reinsert operations

        // 1. Move a node forward in the list (20 -> 21)
        let _ = list.change_key_of_node(idx_2, RoundedTen(21))?;

        // 2. Move a node backward in the list (50 -> 50)
        let _ = list.change_key_of_node(idx_5, RoundedTen(50))?;

        // 3. Move the head to the middle (10 -> 11)
        let _ = list.change_key_of_node(idx_1, RoundedTen(11))?;

        // Verify final order is 15, 25, 30, 35, 40
        let expected_keys: Vec<_> = [11, 21, 30, 40, 50].into_iter().map(RoundedTen).collect();
        let expected_values = ["Ten", "Twenty", "Thirty", "Forty", "Fifty"];

        let mut cursor = list.first();
        let mut key_idx = 0;

        while cursor.is_some() {
            let node = list.get_at(cursor.unwrap());
            assert_eq!(node.unwrap().0, &expected_keys[key_idx]);
            assert_eq!(node.unwrap().1, &expected_values[key_idx]);

            key_idx += 1;
            cursor = list.next_pos(cursor);
        }

        // Verify the prev/next pointers are correct by reverse traversal
        let mut cursor = list.last();
        let mut key_idx = 4;

        while cursor.is_some() {
            let node = list.get_at(cursor.unwrap());
            assert_eq!(node.unwrap().0, &expected_keys[key_idx]);
            assert_eq!(node.unwrap().1, &expected_values[key_idx]);

            cursor = list.prev_pos(cursor);
            if key_idx > 0 {
                key_idx -= 1;
            } else {
                break;
            }
        }
    }
    Ok(())
}

#[test]
fn test_insert_with_hint() -> Result<(), CppMapError> {
    for _i in 0..100 {
        // Test how key changes are handled when the new key would normally change the ordering
        let mut list = SkipList::default();

        // Insert nodes with some space between them
        let idx_10 = list.insert(RoundedTen(10), "Ten")?;
        let idx_20 = list.insert_with_hint(RoundedTen(20), "Twenty", idx_10)?;
        let idx_30 = list.insert_with_hint(RoundedTen(30), "Thirty", idx_20)?;
        let _idx_40 = list.insert_with_hint(RoundedTen(40), "Forty", idx_30)?;
    }
    Ok(())
}
