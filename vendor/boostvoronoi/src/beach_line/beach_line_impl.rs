// SPDX-License-Identifier:BSL-1.0

// Boost.Polygon library detail/voronoi_structures.hpp header file

//          Copyright Andrii Sydorchuk 2010-2012.
// Distributed under the Boost Software License, Version 1.0.
//    (See accompanying file LICENSE_1_0.txt or copy at
//          http://www.boost.org/LICENSE_1_0.txt)

// See http://www.boost.org for updates, documentation, and revision history of C++ code.

// Ported from C++ boost 1.76.0 to Rust in 2020/2021,2025 by Eadf (github.com/eadf)

use super::{
    BeachLine, BeachLineIndex, BeachLineNodeData, BeachLineNodeDataType, BeachLineNodeKey,
    BeachlineCursor,
};
use crate::{BvError, InputType};
#[cfg(feature = "console_debug")]
use crate::{t, tln};
use super::rbtree::{Index, RbMap};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

impl<I: InputType> BeachLine<I> {
    #[cfg(feature = "console_debug")]
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.beach_line_.borrow().len()
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.beach_line_.borrow().is_empty()
    }

    #[inline(always)]
    pub(crate) fn get_pointer(
        &self,
        new_key_id: BeachLineIndex,
    ) -> Result<BeachlineCursor<I>, BvError> {
        BeachlineCursor::new_from_index(Rc::clone(&self.beach_line_), Some(new_key_id))
    }

    #[inline(always)]
    /// A cursor at the last beach-line node (`--end()`)
    pub(crate) fn last(&self) -> Result<BeachlineCursor<I>, BvError> {
        BeachlineCursor::new_from_index(
            Rc::clone(&self.beach_line_),
            self.beach_line_.borrow().last(),
        )
    }

    /// `beach_line_.insert(position, (key, data))`, `position` a node or None for `end()`:
    /// the inserted node, or the node already holding an equivalent key, as std::map's
    /// hinted insert (and its probes) does.
    #[cfg(not(feature = "console_debug"))]
    pub(crate) fn insert(
        &mut self,
        position: Option<BeachLineIndex>,
        key: BeachLineNodeKey<I>,
        data: Option<BeachLineNodeData>,
    ) -> Result<(BeachLineNodeKey<I>, BeachLineIndex), BvError> {
        let node = Rc::new(Cell::new(data));
        let node_index = self
            .beach_line_
            .try_borrow_mut()?
            .insert_with_hint(position, key, node);
        Ok((key, node_index))
    }

    /// As the non-debug `insert`, printing the node.
    #[cfg(feature = "console_debug")]
    pub(crate) fn insert(
        &mut self,
        position: Option<BeachLineIndex>,
        key: BeachLineNodeKey<I>,
        data: Option<BeachLineNodeData>,
        _ce: &crate::circle_event::CircleEventQueue,
    ) -> Result<(BeachLineNodeKey<I>, BeachLineIndex), BvError> {
        let node = Rc::new(Cell::new(data));
        let node_clone = node.clone();
        let node_index = self
            .beach_line_
            .try_borrow_mut()?
            .insert_with_hint(position, key, node);
        t!("inserted beach_line:");
        self.dbgpa_compat_node_(&key, &node_clone, _ce)?;
        Ok((key, node_index))
    }

    /// Clear the beach line
    #[inline(always)]
    pub fn clear(&mut self) {
        self.beach_line_.borrow_mut().clear();
    }

    /// mapping: BeachLineIndexType->(BeachLineNodeKey,BeachLineNodeDataType)
    pub(crate) fn get_node(
        &self,
        beachline_index: &BeachLineIndex,
    ) -> Result<(BeachLineNodeKey<I>, BeachLineNodeDataType), BvError> {
        let bl_borrow = self.beach_line_.try_borrow()?;
        let (k, v) = bl_borrow.get(*beachline_index).ok_or_else(|| {
            BvError::InternalError(format!(
                "Tried to retrieve a beach line node that doesn't exist. Id:{}. {}:{}",
                beachline_index,
                file!(),
                line!()
            ))
        })?;
        Ok((*k, Rc::clone(v)))
    }

    #[inline(always)]
    /// `lower_bound(key)`: the first node not less than `key`; a cursor at `end()` (is_ok()
    /// false) when there is none.
    pub(crate) fn lower_bound(
        &self,
        key: BeachLineNodeKey<I>,
    ) -> Result<BeachlineCursor<I>, BvError> {
        BeachlineCursor::lower_bound(Rc::clone(&self.beach_line_), key)
    }

    #[allow(dead_code)]
    #[cfg(feature = "console_debug")]
    pub(crate) fn debug_cmp_all(&self, key: BeachLineNodeKey<I>) {
        use crate::predicate::node_comparison_predicate;
        for (i, (k, _)) in self.beach_line_.borrow().iter().collect::<Vec<_>>().into_iter().rev().enumerate() {
            t!("#{}:", i);
            let _rv = node_comparison_predicate::node_comparison::<I>(k, &key);
        }
    }

    #[cfg(feature = "console_debug")]
    #[allow(dead_code)]
    pub(crate) fn debug_print_all(&self) -> Result<(), BvError> {
        tln!();
        tln!("beach_line.len()={}", self.beach_line_.borrow().len());
        for (i, (node_key, node_data)) in self.beach_line_.borrow().iter().collect::<Vec<_>>().into_iter().rev().enumerate() {
            t!(
                "beach_line{} L:{:?},R:{:?}",
                i,
                &node_key.left_site(),
                &node_key.right_site()
            );

            if let Some(data) = node_data.get() {
                if let Some(circle_event) = data.circle_event_ {
                    t!(" -> CircleEvent:{}", circle_event);
                } else {
                    t!(" -> CircleEvent:-");
                }
                t!(", edge:{:?}", data.edge_);
            } else {
                t!(" temporary bisector");
            }
            tln!();
        }
        tln!();
        Ok(())
    }

    #[cfg(feature = "console_debug")]
    pub(crate) fn dbgpa_compat_(
        &self,
        ce: &crate::circle_event::CircleEventQueue,
    ) -> Result<(), BvError> {
        tln!("-----beach_line----{}", self.beach_line_.borrow().len());
        for (i, (node_key, node_data)) in self.beach_line_.borrow().iter().enumerate() {
            t!("#{}:", i);
            self.dbgpa_compat_node_(node_key, node_data, ce)?;
        }
        tln!();
        Ok(())
    }

    #[cfg(feature = "console_debug")]
    pub(crate) fn dbgp_all_cmp(&self) {
        let _iter1 = self.beach_line_.borrow();
        let mut it1 = _iter1.iter().enumerate();
        for it2_v in self.beach_line_.borrow().iter().enumerate().skip(1) {
            let it1_v = it1.next().unwrap();
            let key1 = it1_v.1.0;
            let key2 = it2_v.1.0;
            t!(
                "key(#{}).partial_cmp(key(#{})) == {:?}",
                it1_v.0,
                it2_v.0,
                key1.partial_cmp(key2).unwrap()
            );
            tln!(
                "\tkey(#{}).partial_cmp(key(#{})) == {:?}",
                it2_v.0,
                it1_v.0,
                key2.partial_cmp(key1).unwrap()
            );
        }
    }

    #[cfg(feature = "console_debug")]
    pub(crate) fn dbgpa_compat_node_(
        &self,
        node_key: &BeachLineNodeKey<I>,
        node_data: &BeachLineNodeDataType,
        ce: &crate::circle_event::CircleEventQueue,
    ) -> Result<(), BvError> {
        t!(
            "L:{:?},R:{:?}",
            &node_key.left_site(),
            &node_key.right_site(),
        );
        if let Some(data) = node_data.get() {
            if let Some(_circle_event) = data.circle_event_ {
                if ce.is_active(_circle_event) {
                    t!(" -> CircleEvent: ");
                    ce.dbg_ce(_circle_event);
                } else {
                    t!(" -> CircleEvent=-"); // this is what the c++ code does, should print "inactive"
                }
            } else {
                t!(" -> CircleEvent=-");
            }
            t!(",e={:?}", data.edge_);
        } else {
            t!(" Temporary bisector");
        }

        // t!(" id={}", id);
        tln!();
        Ok(())
    }
}

impl<I: InputType> BeachLineNodeKey<I> {
    /// Constructs degenerate bisector, used to search an arc that is above
    /// the given site. The input to the constructor is the new site point.
    pub fn new_1(new_site: crate::site_event::SiteEvent<I>) -> Self {
        Self {
            left_site_: new_site,
            right_site_: new_site,
        }
    }

    /// Constructs a new bisector. The input to the constructor is the two
    /// sites that create the bisector. The order of sites is important.
    pub fn new_2(
        left_site: crate::site_event::SiteEvent<I>,
        right_site: crate::site_event::SiteEvent<I>,
    ) -> Self {
        Self {
            left_site_: left_site,
            right_site_: right_site,
        }
    }

    #[inline(always)]
    pub(crate) fn left_site(&self) -> &crate::site_event::SiteEvent<I> {
        &self.left_site_
    }

    #[inline(always)]
    pub(crate) fn right_site(&self) -> &crate::site_event::SiteEvent<I> {
        &self.right_site_
    }

    #[inline(always)]
    pub(crate) fn set_right_site(&mut self, site: &crate::site_event::SiteEvent<I>) {
        self.right_site_ = *site; // Copy
    }
}

impl BeachLineNodeData {
    pub fn new_1(new_edge: crate::diagram::EdgeIndex) -> Self {
        Self {
            circle_event_: None,
            edge_: new_edge,
        }
    }

    #[inline(always)]
    pub fn get_circle_event_id(&self) -> Option<crate::circle_event::CircleEventIndex> {
        self.circle_event_
    }

    #[inline(always)]
    pub(crate) fn set_circle_event_id(
        &mut self,
        circle_event: Option<crate::circle_event::CircleEventIndex>,
    ) {
        self.circle_event_ = circle_event;
    }

    #[inline(always)]
    pub(crate) fn edge_id(&self) -> crate::diagram::EdgeIndex {
        self.edge_
    }

    #[inline(always)]
    pub(crate) fn set_edge_id(&mut self, new_edge: crate::diagram::EdgeIndex) {
        self.edge_ = new_edge;
    }
}

impl<I: InputType> BeachlineCursor<I> {
    // K= BeachLineNodeKey<I>
    // V= BeachLineNodeDataType

    /// A cursor at `begin()`.
    #[allow(dead_code)]
    pub(crate) fn new_1(
        list: Rc<RefCell<RbMap<BeachLineNodeKey<I>, BeachLineNodeDataType>>>,
    ) -> Result<Self, BvError> {
        let head = list.try_borrow()?.first();
        Ok(Self {
            current: head,
            list,
        })
    }

    pub(crate) fn new_2(
        list: Rc<RefCell<RbMap<BeachLineNodeKey<I>, BeachLineNodeDataType>>>,
        pos: Index,
    ) -> Result<Self, BvError> {
        Ok(Self {
            current: Some(pos),
            list,
        })
    }

    /// The node, or None at `end()`.
    #[inline(always)]
    pub fn current(&self) -> Option<Index> {
        self.current
    }

    #[inline(always)]
    pub fn current_must_be_valid(&self) -> Result<Index, BvError> {
        self.current.ok_or_else(|| {
            BvError::IndexError(format!(
                "current_must_be_valid: was None when Some was expected {} {}",
                file!(),
                line!()
            ))
        })
    }

    /// A cursor at a node.
    pub fn new_from_index(
        list: Rc<RefCell<RbMap<BeachLineNodeKey<I>, BeachLineNodeDataType>>>,
        current: Option<BeachLineIndex>,
    ) -> Result<Self, BvError> {
        if current.is_some() {
            Ok(Self { current, list })
        } else {
            Err(BvError::IndexError(format!(
                "new_from_index: was None when Some was expected {} {}",
                file!(),
                line!()
            )))
        }
    }

    #[inline(always)]
    /// A copy of the key at the cursor.
    pub fn get_k(&self) -> Result<BeachLineNodeKey<I>, BvError> {
        let list = self.list.try_borrow()?;
        self.current
            .and_then(|c| list.get(c))
            .map(|(k, _)| *k)
            .ok_or_else(|| {
                BvError::IndexError(format!(
                    "get_k: was None when Some was expected {} {}",
                    file!(),
                    line!()
                ))
            })
    }

    #[inline(always)]
    /// A clone of the value at the cursor.
    pub fn get_v(&self) -> Result<BeachLineNodeDataType, BvError> {
        let list = self.list.try_borrow()?;
        self.current
            .and_then(|c| list.get(c))
            .map(|(_, v)| v.clone())
            .ok_or_else(|| {
                BvError::IndexError(format!(
                    "get_v: was None when Some was expected {} {}",
                    file!(),
                    line!()
                ))
            })
    }

    #[allow(clippy::should_implement_trait)]
    #[inline(always)]
    /// `++it`; `end()` past the last node.
    /// Note that this is NOT a Rust iterator next() method.
    pub fn next(&mut self) -> Result<(), BvError> {
        let list_borrow = self.list.try_borrow()?;
        self.current = list_borrow.next(self.current);
        Ok(())
    }

    #[inline(always)]
    /// `--it`; from `end()` the last node.
    pub fn prev(&mut self) -> Result<(), BvError> {
        let list_borrow = self.list.try_borrow()?;
        self.current = list_borrow.prev(self.current);
        Ok(())
    }

    #[allow(dead_code)]
    #[inline(always)]
    /// Move to the first node
    pub fn move_to_head(&mut self) -> Result<(), BvError> {
        self.current = self.list.try_borrow()?.first();
        Ok(())
    }

    #[allow(dead_code)]
    #[inline(always)]
    /// Move to the last node
    pub fn move_to_tail(&mut self) -> Result<(), BvError> {
        self.current = self.list.try_borrow()?.last();
        Ok(())
    }

    #[inline(always)]
    /// Whether the cursor is at a node, not `end()`.
    pub fn is_ok(&self) -> Result<bool, BvError> {
        let list = self.list.try_borrow()?;
        Ok(self.current.is_some_and(|c| list.get(c).is_some()))
    }

    #[inline(always)]
    /// `it == begin()`; true for an empty beach line as there begin() == end().
    pub fn is_at_head(&self) -> Result<bool, BvError> {
        Ok(self.current == self.list.try_borrow()?.first())
    }

    #[allow(dead_code)]
    #[inline(always)]
    /// Whether the cursor is at the last node.
    pub fn is_at_tail(&self) -> Result<bool, BvError> {
        let list = self.list.try_borrow()?;
        Ok(self.current.is_some() && self.current == list.last())
    }

    #[inline(always)]
    /// Change the key at the cursor in place, without reordering: Boost's
    /// `const_cast<key_type&>(it->first)`. The caller keeps the beach line ordered.
    pub fn set_key_in_place(&mut self, new_key: BeachLineNodeKey<I>) -> Result<(), BvError> {
        let mut list = self.list.try_borrow_mut()?;
        let key = self.current.and_then(|c| list.key_mut(c)).ok_or_else(|| {
            BvError::IndexError(format!(
                "set_key_in_place: no node at the cursor {} {}",
                file!(),
                line!()
            ))
        })?;
        *key = new_key;
        Ok(())
    }

    #[inline(always)]
    /// `beach_line_.erase(it)`; the cursor is then at `end()`.
    /// Note: make sure that there are no other cursors at this position.
    pub fn remove_current(&mut self) -> Result<(), BvError> {
        let removed = match self.current {
            Some(c) => self.list.try_borrow_mut()?.erase(c),
            None => false,
        };
        if !removed {
            return Err(BvError::InternalError(format!(
                "Could not remove beach line node {:?} {}:{}",
                self.current,
                file!(),
                line!()
            )));
        }
        self.current = None;
        Ok(())
    }

    #[inline(always)]
    /// A cursor at `lower_bound(key)`, at `end()` (is_ok() false) when no node is not less.
    pub fn lower_bound(
        list: Rc<RefCell<RbMap<BeachLineNodeKey<I>, BeachLineNodeDataType>>>,
        key: BeachLineNodeKey<I>,
    ) -> Result<Self, BvError> {
        let position = list.try_borrow()?.lower_bound(&key);
        Ok(Self {
            list,
            current: position,
        })
    }
}

/// debug utility function, prints beach line index
#[allow(dead_code)]
#[cfg(feature = "console_debug")]
#[inline(always)]
pub(crate) fn debug_print_bli_id(value: Option<BeachLineIndex>) -> String {
    if let Some(value) = value {
        value.to_string()
    } else {
        String::from("-")
    }
}
