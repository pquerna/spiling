// SPDX-License-Identifier: MIT OR Apache-2.0

// Copyright 2025 Eadf (github.com/eadf)
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::linkedlist::LinkedList;
use crate::prelude::CppMapError;

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_insert() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();
    let _1 = list.insert(1, 1.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_1.0));
    let _2 = list.insert(2, 2.0)?;
    assert_eq!(list.mid_point_delta, 1);
    assert_eq!(list.mid_point, Some(_1.0));
    let _3 = list.insert(3, 3.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_2.0));
    let _4 = list.insert(4, 4.0)?;
    assert_eq!(list.mid_point_delta, 1);
    assert_eq!(list.mid_point, Some(_2.0));
    let _5 = list.insert(5, 5.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_3.0));
    let _6 = list.insert(6, 6.0)?;
    assert_eq!(list.mid_point_delta, 1);
    assert_eq!(list.mid_point, Some(_3.0));
    let _7 = list.insert(7, 7.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_4.0));
    let _8 = list.insert(8, 8.0)?;
    assert_eq!(list.mid_point_delta, 1);
    assert_eq!(list.mid_point, Some(_4.0));
    let _9 = list.insert(9, 9.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_5.0));
    let _0 = list.insert(0, 0.0)?;
    assert_eq!(list.mid_point_delta, -1);
    assert_eq!(list.mid_point, Some(_5.0));
    Ok(())
}

#[test]
fn test_midpoint_insert_edge_cases() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();

    // === Test 1: Insert at empty list ===
    let k1 = list.insert(10, 1.0)?;
    assert_eq!(list.mid_point, Some(k1.0));
    assert_eq!(list.mid_point_delta, 0);

    // === Test 2: Insert at tail (right of mid) ===
    let _k2 = list.insert(20, 2.0)?; // Right of 10 (delta=1)
    assert_eq!(list.mid_point, Some(k1.0));
    assert_eq!(list.mid_point_delta, 1);

    // === Test 3: Insert at head (left of mid) ===
    let _k0 = list.insert(5, 0.5)?; // Left of 10 (delta=0)
    assert_eq!(list.mid_point, Some(k1.0));
    assert_eq!(list.mid_point_delta, 0);

    // === Test 4: Insert just left of mid ===
    let _k_mid_left = list.insert(9, 0.9)?; // Left of 10 (delta=-1)
    assert_eq!(list.mid_point, Some(k1.0));
    assert_eq!(list.mid_point_delta, -1);

    // === Test 5: Insert just right of mid ===
    let _k_mid_right = list.insert(11, 1.1)?; // Right of 10 (delta=0)
    assert_eq!(list.mid_point, Some(k1.0));
    assert_eq!(list.mid_point_delta, 0);

    // === Test 6: Force mid move right (delta > 1) ===
    let _ = list.insert(21, 2.1)?; // Right of 10 (delta=1)
    let _ = list.insert(22, 2.2)?; // Right of 10 (delta=2 → move mid to 11)
    list.debug_print();
    assert_eq!(list.mid_point, Some(_k_mid_right.0));
    assert_eq!(list.mid_point_delta, 0);

    // === Test 7: Force mid move left (delta < -1) ===
    let _ = list.insert(8, 0.8)?; // Left of 20 (delta=-1)
    let _ = list.insert(7, 0.7)?; // Left of 20 (delta=-2 → move mid to 10)
    assert_eq!(list.mid_point, Some(k1.0));
    assert_eq!(list.mid_point_delta, 0);

    // === Test 8: Insert at new tail after mid moves ===
    let _k_new_tail = list.insert(30, 3.0)?; // Right of 20 (delta=1)
    assert_eq!(list.mid_point, Some(k1.0));
    assert_eq!(list.mid_point_delta, 1);

    // === Test 9: Insert at new head after mid moves ===
    let _k_new_head = list.insert(1, 0.1)?; // Left of 5 (delta=0)
    assert_eq!(list.mid_point, Some(k1.0));
    assert_eq!(list.mid_point_delta, 0);

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_remove() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();
    let _1 = list.insert(1, 1.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_1.0));
    let _2 = list.insert(2, 2.0)?;
    assert_eq!(list.mid_point_delta, 1);
    assert_eq!(list.mid_point, Some(_1.0));
    let _3 = list.insert(3, 3.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_2.0));
    let mut index = Some(_1);

    let _ = list.remove_by_index(&mut index);
    list.debug_print();
    assert_eq!(index.unwrap().0, _2.0); // index cursor moved to 2
    assert_eq!(list.mid_point_delta, 1);
    assert_eq!(list.mid_point, Some(_2.0));

    let _ = list.remove_by_index(&mut index);
    list.debug_print();
    assert_eq!(index.unwrap().0, _3.0); // index cursor moved to 3
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_3.0));

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_remove_basic() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();
    let _1 = list.insert(1, 1.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_1.0));

    let _2 = list.insert(2, 2.0)?;
    assert_eq!(list.mid_point_delta, 1);
    assert_eq!(list.mid_point, Some(_1.0));

    let _3 = list.insert(3, 3.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_2.0));

    let mut index = Some(_1);

    let _ = list.remove_by_index(&mut index);
    list.validate();
    assert_eq!(index.unwrap().0, _2.0); // index cursor moved to 2
    assert_eq!(list.mid_point_delta, 1);
    assert_eq!(list.mid_point, Some(_2.0));

    let _ = list.remove_by_index(&mut index);
    list.validate();
    assert_eq!(index.unwrap().0, _3.0); // index cursor moved to 3
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_3.0));

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_remove_single_element() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();
    let _1 = list.insert(1, 1.0)?;
    assert_eq!(list.mid_point_delta, 0);
    assert_eq!(list.mid_point, Some(_1.0));

    let mut index = Some(_1);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    //assert_eq!(index, None); // No more elements
    assert_eq!(list.mid_point, None); // Midpoint should be none
    assert_eq!(list.mid_point_delta, 0);
    assert!(list.is_empty());

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_remove_middle_element() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();
    let _1 = list.insert(1, 1.0)?;
    let _2 = list.insert(2, 2.0)?;
    let _3 = list.insert(3, 3.0)?;
    let _4 = list.insert(4, 4.0)?;
    let _5 = list.insert(5, 5.0)?;

    // Midpoint should be at element 3
    assert_eq!(list.mid_point, Some(_3.0));
    assert_eq!(list.mid_point_delta, 0);

    // Remove the midpoint element
    let mut index = Some(_3);
    let _ = list.remove_by_index(&mut index);
    list.validate();
    // Index should move to element 2
    assert_eq!(index.unwrap().0, _2.0);

    // New midpoint should be either 2 or 4, depending on implementation preference
    // Our implementation prefers moving forward when possible
    assert_eq!(list.mid_point, Some(_4.0));

    // Since we moved to the next element (right), delta should decrease by 1
    assert_eq!(list.mid_point_delta, -1);

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_remove_head_element() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();
    let _1 = list.insert(1, 1.0)?;
    let _2 = list.insert(2, 2.0)?;
    let _3 = list.insert(3, 3.0)?;

    // Make sure midpoint is set up correctly
    assert_eq!(list.mid_point, Some(_2.0));
    assert_eq!(list.mid_point_delta, 0);

    // Remove the first element
    let mut index = Some(_1);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // Index should move to element 2
    assert_eq!(index.unwrap().0, _2.0);

    // Midpoint should now be element 2
    assert_eq!(list.mid_point, Some(_2.0));

    // Delta should be updated (one less element before midpoint)
    assert_eq!(list.mid_point_delta, 1);

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_remove_tail_element() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();
    let _1 = list.insert(1, 1.0)?;
    let _2 = list.insert(2, 2.0)?;
    let _3 = list.insert(3, 3.0)?;

    // Make sure midpoint is set up correctly
    assert_eq!(list.mid_point, Some(_2.0));
    assert_eq!(list.mid_point_delta, 0);

    // Remove the last element
    let mut index = Some(_3);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // Index should move to element 2 (previous element)
    assert_eq!(index.unwrap().0, _2.0);

    // Midpoint should still be element 2
    assert_eq!(list.mid_point, Some(_2.0));

    // Delta should be updated (one less element after midpoint)
    assert_eq!(list.mid_point_delta, -1);

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_imbalanced_insertions() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();

    // Insert elements in increasing order
    let _1 = list.insert(1, 1.0)?;
    let _2 = list.insert(2, 2.0)?;
    let _3 = list.insert(3, 3.0)?;
    let _4 = list.insert(4, 4.0)?;
    let _5 = list.insert(5, 5.0)?;

    // Midpoint should shift as we insert more elements
    assert_eq!(list.mid_point, Some(_3.0));
    assert_eq!(list.mid_point_delta, 0);

    // Insert more elements on the right side
    let _6 = list.insert(6, 6.0)?;
    list.validate();
    let _7 = list.insert(7, 7.0)?;
    list.validate();

    // After two right-side insertions, midpoint should move right once
    assert_eq!(list.mid_point, Some(_4.0));
    assert_eq!(list.mid_point_delta, 0);

    // Insert more elements on the left side
    let _0 = list.insert(0, 0.0)?;
    list.validate();
    let _m1 = list.insert(-1, -1.0)?;
    list.validate();

    // After two left-side insertions, midpoint should move left once
    assert_eq!(list.mid_point, Some(_3.0));
    assert_eq!(list.mid_point_delta, 0);

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_alternating_insertions() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();

    // Insert elements in alternating order (odd-even)
    let _1 = list.insert(1, 1.0)?;
    assert_eq!(list.mid_point, Some(_1.0));
    assert_eq!(list.mid_point_delta, 0);

    let _3 = list.insert(3, 3.0)?;
    assert_eq!(list.mid_point, Some(_1.0));
    assert_eq!(list.mid_point_delta, 1);

    let _2 = list.insert(2, 2.0)?;
    // Midpoint should shift right because we inserted between midpoint and next
    assert_eq!(list.mid_point, Some(_2.0));
    assert_eq!(list.mid_point_delta, 0);

    let _5 = list.insert(5, 5.0)?;
    assert_eq!(list.mid_point, Some(_2.0));
    assert_eq!(list.mid_point_delta, 1);

    let _4 = list.insert(4, 4.0)?;
    // Midpoint should shift right again
    assert_eq!(list.mid_point, Some(_3.0));
    assert_eq!(list.mid_point_delta, 0);

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_sequential_removals() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();

    // Insert elements
    let _1 = list.insert(1, 1.0)?;
    let _2 = list.insert(2, 2.0)?;
    let _3 = list.insert(3, 3.0)?;
    let _4 = list.insert(4, 4.0)?;
    let _5 = list.insert(5, 5.0)?;

    // Midpoint should be at element 3
    assert_eq!(list.mid_point, Some(_3.0));
    assert_eq!(list.mid_point_delta, 0);

    // Remove elements from the beginning
    let mut index = Some(_1);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // After removing element 1, midpoint should still be 3
    assert_eq!(list.mid_point, Some(_3.0));
    assert_eq!(list.mid_point_delta, 1); // One less element before midpoint
    // Remove element 2
    assert_eq!(index.unwrap().0, _2.0);
    let _ = list.remove_by_index(&mut index);
    list.validate();
    // After removing element 2, midpoint should shift left to maintain balance
    assert_eq!(list.mid_point, Some(_4.0));
    assert_eq!(list.mid_point_delta, 0); // Balanced after midpoint adjustment

    // Remove element 3 (which is the midpoint)
    assert_eq!(index.unwrap().0, _3.0);
    let _ = list.remove_by_index(&mut index);
    list.validate();
    // After removing midpoint, a new midpoint should be selected
    assert_eq!(index.unwrap().0, _4.0);
    assert_eq!(list.mid_point, Some(_4.0));
    assert_eq!(list.mid_point_delta, 1);

    // Remove element 4 (the new midpoint)
    let _ = list.remove_by_index(&mut index);
    list.validate();
    // After removing the midpoint again, element 5 should be the only one left
    assert_eq!(index.unwrap().0, _5.0);
    assert_eq!(list.mid_point, Some(_5.0));
    assert_eq!(list.mid_point_delta, 0);

    // Remove the last element
    let _ = list.remove_by_index(&mut index);
    list.validate();
    // List should be empty now
    //assert_eq!(index, list.last());
    assert_eq!(list.mid_point, None);
    assert_eq!(list.mid_point_delta, 0);
    assert!(list.is_empty());

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_random_order_removals() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();

    // Insert elements
    let _1 = list.insert(1, 1.0)?;
    let _2 = list.insert(2, 2.0)?;
    let _3 = list.insert(3, 3.0)?;
    let _4 = list.insert(4, 4.0)?;
    let _5 = list.insert(5, 5.0)?;

    // Midpoint should be at element 3
    assert_eq!(list.mid_point, Some(_3.0));
    assert_eq!(list.mid_point_delta, 0);

    // Remove element 4 (after midpoint)
    let mut index = Some(_4);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // Index should move to element 5 or 3 (depends on implementation)
    // Let's check it moved to one of them
    let valid_next_indices = [_3.0, _5.0];
    assert!(valid_next_indices.contains(&index.unwrap().0));

    // Midpoint should still be element 3
    assert_eq!(list.mid_point, Some(_3.0));
    assert_eq!(list.mid_point_delta, -1); // One less element after midpoint

    // Remove element 2 (before midpoint)
    index = Some(_2);
    let _ = list.remove_by_index(&mut index);

    // Delta should be 0 again (balanced)
    assert_eq!(list.mid_point_delta, 0);

    // Now remove the midpoint (element 3)
    index = Some(_3);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // Index should move to 5
    assert_eq!(list.mid_point, Some(_5.0));
    assert_eq!(list.mid_point_delta, -1);

    Ok(())
}

#[allow(clippy::just_underscores_and_digits)]
#[test]
fn test_midpoint_edge_case_rebalancing() -> Result<(), CppMapError> {
    let mut list = LinkedList::default();

    // Create a highly imbalanced list
    let _1 = list.insert(1, 1.0)?;
    let _2 = list.insert(2, 2.0)?;
    let _3 = list.insert(3, 3.0)?;
    let _4 = list.insert(4, 4.0)?;
    let _5 = list.insert(5, 5.0)?;
    let _6 = list.insert(6, 6.0)?;
    let _7 = list.insert(7, 7.0)?;

    // At this point, midpoint should have adjusted to element 4
    assert_eq!(list.mid_point, Some(_4.0));
    assert_eq!(list.mid_point_delta, 0);

    // Now remove elements 2, 3, 5, 6 to create an imbalanced situation
    // First remove 2
    let mut index = Some(_2);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // Then remove 3
    index = Some(_3);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // Then remove 5
    index = Some(_5);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // Then remove 6
    index = Some(_6);
    let _ = list.remove_by_index(&mut index);
    list.validate();

    // Now we have only elements 1, 4, 7
    // Midpoint should be element 4
    assert_eq!(list.mid_point, Some(_4.0));
    // Remove element 4 (midpoint)
    index = Some(_4);
    let _ = list.remove_by_index(&mut index); // cursor moves to .prev if possible
    list.validate();
    assert_eq!(index, Some(_1));
    assert_eq!(list.mid_point, Some(_7.0));
    assert_eq!(list.mid_point_delta, -1);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::just_underscores_and_digits)]
    #[test]
    fn test_midpoint_edge_case_rebalancing() -> Result<(), CppMapError> {
        let mut list = LinkedList::default();

        // Create a highly imbalanced list
        let _1 = list.insert(1, 1.0)?;
        let _2 = list.insert(2, 2.0)?;
        let _3 = list.insert(3, 3.0)?;
        let _4 = list.insert(4, 4.0)?;
        let _5 = list.insert(5, 5.0)?;
        let _6 = list.insert(6, 6.0)?;
        let _7 = list.insert(7, 7.0)?;

        // At this point, midpoint should have adjusted to element 4
        assert_eq!(list.mid_point, Some(_4.0));
        assert_eq!(list.mid_point_delta, 0);

        // Now remove elements 2, 3, 5, 6 to create an imbalanced situation
        // First remove 2
        let mut index = Some(_2);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Then remove 3
        index = Some(_3);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Then remove 5
        index = Some(_5);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Then remove 6
        index = Some(_6);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Now we have only elements 1, 4, 7
        // Midpoint should be element 4
        assert_eq!(list.mid_point, Some(_4.0));
        // Remove element 4 (midpoint)
        index = Some(_4);
        let _ = list.remove_by_index(&mut index); // cursor moves to .prev if possible
        list.validate();
        assert_eq!(index, Some(_1));
        assert_eq!(list.mid_point, Some(_7.0));
        assert_eq!(list.mid_point_delta, -1);

        Ok(())
    }

    #[allow(clippy::just_underscores_and_digits)]
    #[test]
    fn test_midpoint_removal_left_heavy() -> Result<(), CppMapError> {
        // Test removing midpoint in a left-heavy list
        let mut list = LinkedList::default();

        // Create a list with 5 elements
        let _1 = list.insert(1, 1.0)?;
        let _2 = list.insert(2, 2.0)?;
        let _3 = list.insert(3, 3.0)?;
        let _4 = list.insert(4, 4.0)?;
        let _5 = list.insert(5, 5.0)?;

        // Verify initial midpoint
        assert_eq!(list.mid_point, Some(_3.0));
        assert_eq!(list.mid_point_delta, 0);

        // Remove elements 4 and 5 to make it left-heavy
        let mut index = Some(_4);
        let _ = list.remove_by_index(&mut index);
        list.validate();
        index = Some(_5);

        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Verify state before midpoint removal
        assert_eq!(list.mid_point, Some(_2.0));
        assert_eq!(list.mid_point_delta, 0);

        // Now remove the midpoint
        index = Some(_2);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Should prefer moving right when left-heavy
        assert_eq!(list.mid_point, Some(_3.0));
        assert_eq!(list.mid_point_delta, -1);

        Ok(())
    }

    #[allow(clippy::just_underscores_and_digits)]
    #[test]
    fn test_midpoint_removal_right_heavy() -> Result<(), CppMapError> {
        // Test removing midpoint in a right-heavy list
        let mut list = LinkedList::default();

        // Create a list with 5 elements
        let _1 = list.insert(1, 1.0)?;
        let _2 = list.insert(2, 2.0)?;
        let _3 = list.insert(3, 3.0)?;
        let _4 = list.insert(4, 4.0)?;
        let _5 = list.insert(5, 5.0)?;

        // Verify initial midpoint
        assert_eq!(list.mid_point, Some(_3.0));
        assert_eq!(list.mid_point_delta, 0);

        // Remove elements 1 and 2 to make it right-heavy
        let mut index = Some(_1);
        let _ = list.remove_by_index(&mut index);
        list.validate();
        index = Some(_2);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Verify state before midpoint removal
        assert_eq!(list.mid_point, Some(_4.0));
        assert_eq!(list.mid_point_delta, 0);

        // Now remove the midpoint
        index = Some(_4);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        assert_eq!(list.mid_point, Some(_5.0));
        assert_eq!(list.mid_point_delta, -1);

        Ok(())
    }

    #[allow(clippy::just_underscores_and_digits)]
    #[test]
    fn test_midpoint_removal_edge_cases() -> Result<(), CppMapError> {
        // Test edge cases when removing midpoint
        let mut list = LinkedList::default();

        // Create a list with just 3 elements
        let _1 = list.insert(1, 1.0)?;
        list.validate();
        let _2 = list.insert(2, 2.0)?;
        list.validate();
        let _3 = list.insert(3, 3.0)?;
        list.validate();

        // Verify initial midpoint
        assert_eq!(list.mid_point, Some(_2.0));
        assert_eq!(list.mid_point_delta, 0);

        // Now remove the midpoint
        let mut index = Some(_2);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // With just 2 elements, midpoint could be either one
        // Let's verify it picked one and delta makes sense
        assert!(list.mid_point == Some(_1.0) || list.mid_point == Some(_3.0));
        if list.mid_point == Some(_1.0) {
            assert_eq!(list.mid_point_delta, 1);
        } else {
            assert_eq!(list.mid_point_delta, -1);
        }

        Ok(())
    }

    #[allow(clippy::just_underscores_and_digits)]
    #[test]
    fn test_midpoint_removal_series() -> Result<(), CppMapError> {
        // Test a series of midpoint removals
        let mut list = LinkedList::default();

        // Create a larger list with 7 elements
        let _1 = list.insert(1, 1.0)?;
        let _2 = list.insert(2, 2.0)?;
        let _3 = list.insert(3, 3.0)?;
        let _4 = list.insert(4, 4.0)?;
        let _5 = list.insert(5, 5.0)?;
        let _6 = list.insert(6, 6.0)?;
        let _7 = list.insert(7, 7.0)?;

        // Verify initial midpoint
        assert_eq!(list.mid_point, Some(_4.0));
        assert_eq!(list.mid_point_delta, 0);

        // Now remove the midpoint repeatedly
        let mut index = Some(_4);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // After first midpoint removal
        // Could select either 3 or 5, but should prefer 5 if balanced
        assert_eq!(list.mid_point, Some(_5.0));
        assert_eq!(list.mid_point_delta, -1);

        // Remove the new midpoint
        index = Some(_5);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Should prefer moving left now since it's right-heavy
        assert_eq!(list.mid_point, Some(_3.0));
        assert_eq!(list.mid_point_delta, 0);

        // Remove the new midpoint
        index = Some(_3);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Should be balanced, could go either way
        assert!(list.mid_point == Some(_2.0) || list.mid_point == Some(_6.0));

        Ok(())
    }

    #[allow(clippy::just_underscores_and_digits)]
    #[test]
    fn test_midpoint_when_head_or_tail_removed() -> Result<(), CppMapError> {
        // Test midpoint behavior when head or tail is removed
        let mut list = LinkedList::default();

        // Create a list with 5 elements
        let _1 = list.insert(1, 1.0)?;
        let _2 = list.insert(2, 2.0)?;
        let _3 = list.insert(3, 3.0)?;
        let _4 = list.insert(4, 4.0)?;
        let _5 = list.insert(5, 5.0)?;

        // Verify initial midpoint
        assert_eq!(list.mid_point, Some(_3.0));
        assert_eq!(list.mid_point_delta, 0);

        // Remove head
        let mut index = Some(_1);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Midpoint should adjust
        assert_eq!(list.mid_point, Some(_3.0));
        assert_eq!(list.mid_point_delta, 1);

        // Remove tail
        index = Some(_5);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // Should be balanced again
        assert_eq!(list.mid_point, Some(_3.0));
        assert_eq!(list.mid_point_delta, 0);

        Ok(())
    }

    #[allow(clippy::just_underscores_and_digits)]
    #[test]
    fn test_midpoint_removal_two_element_list() -> Result<(), CppMapError> {
        // Test removing midpoint in a two-element list
        let mut list = LinkedList::default();

        // Create a list with just 2 elements
        let _1 = list.insert(1, 1.0)?;
        let _2 = list.insert(2, 2.0)?;
        list.validate();

        // Verify initial midpoint (should be the first element)
        assert_eq!(list.mid_point, Some(_1.0));
        assert_eq!(list.mid_point_delta, 1);

        // Now remove the midpoint
        let mut index = Some(_1);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // With just 1 element, midpoint should be that element
        assert_eq!(list.mid_point, Some(_2.0));
        assert_eq!(list.mid_point_delta, 0);

        Ok(())
    }

    #[allow(clippy::just_underscores_and_digits)]
    #[test]
    fn test_midpoint_removal_to_empty_list() -> Result<(), CppMapError> {
        // Test removing the last element (which is also midpoint)
        let mut list = LinkedList::default();

        // Create a list with just 1 element
        let _1 = list.insert(1, 1.0)?;

        // Verify initial midpoint
        assert_eq!(list.mid_point, Some(_1.0));
        assert_eq!(list.mid_point_delta, 0);

        // Now remove the only element
        let mut index = Some(_1);
        let _ = list.remove_by_index(&mut index);
        list.validate();

        // List should be empty, midpoint should be None
        assert_eq!(list.mid_point, None);
        assert_eq!(list.mid_point_delta, 0);
        assert!(list.is_empty());

        Ok(())
    }
}
