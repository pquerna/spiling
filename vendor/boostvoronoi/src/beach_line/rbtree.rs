// SPDX-License-Identifier:BSL-1.0

// The beach line's ordered map, a red-black tree that makes the same comparisons in the same
// order, and keeps the same shape, as the libstdc++ `std::map` (`_Rb_tree`, `stl_tree.h` and
// `tree.cc`) that C++ Boost.Polygon's voronoi builder uses.
//
// Boost's beach line comparator is not a strict weak ordering over every node: it is
// non-monotone across arcs Boost keeps behind the sweep (inverse arcs of segments with no
// extent). A search's answer then depends on which nodes it probes. The reference
// implementation is validated with std::map's probe order; this map reproduces it, node for
// node, so a diagram is the one C++ Boost.Polygon builds. Nothing here is a balanced-tree
// improvement: insertion with and without a hint, lower_bound, erase with its rebalancing and
// iteration follow the libstdc++ algorithms step by step.
//
// Nodes live in an arena and are never reused, so an index held by an inactive circle event
// can never alias a later node (C++ keeps a dangling pointer there, never dereferenced).

use std::fmt;

/// A beach line node's place in the map; stable for the node's life.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Index(usize);

impl fmt::Display for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The map's strict "less than", as std::map's `key_compare`.
pub(crate) trait KeyLess {
    fn key_less(&self, other: &Self) -> bool;
}

const NIL: usize = usize::MAX;
/// The header node, `end()`: its parent is the root, its left the leftmost node and its right
/// the rightmost, as libstdc++'s `_M_header`.
const HEADER: usize = 0;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Color {
    Red,
    Black,
}

struct Node<K, V> {
    parent: usize,
    left: usize,
    right: usize,
    color: Color,
    entry: Option<(K, V)>,
}

pub(crate) struct RbMap<K, V> {
    nodes: Vec<Node<K, V>>,
    len: usize,
}

impl<K, V> Default for RbMap<K, V> {
    fn default() -> Self {
        Self {
            // _M_reset(): the header is red, its parent null, left and right itself.
            nodes: vec![Node {
                parent: NIL,
                left: HEADER,
                right: HEADER,
                color: Color::Red,
                entry: None,
            }],
            len: 0,
        }
    }
}

impl<K: KeyLess, V> RbMap<K, V> {
    #[inline]
    fn n(&self, x: usize) -> &Node<K, V> {
        &self.nodes[x]
    }
    #[inline]
    fn n_mut(&mut self, x: usize) -> &mut Node<K, V> {
        &mut self.nodes[x]
    }
    #[inline]
    fn key(&self, x: usize) -> &K {
        // Only real nodes are compared; the header has no entry.
        &self.nodes[x]
            .entry
            .as_ref()
            .expect("a map node has its entry")
            .0
    }
    #[inline]
    fn less(&self, a: &K, b: &K) -> bool {
        a.key_less(b)
    }
    #[inline]
    fn root(&self) -> usize {
        self.n(HEADER).parent
    }

    #[allow(dead_code)] // tests and console_debug
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// `begin()`: the leftmost node, or None when empty.
    pub(crate) fn first(&self) -> Option<Index> {
        let x = self.n(HEADER).left;
        (x != HEADER).then_some(Index(x))
    }

    /// `--end()`: the rightmost node, or None when empty.
    pub(crate) fn last(&self) -> Option<Index> {
        let x = self.n(HEADER).right;
        (x != HEADER).then_some(Index(x))
    }

    fn live(&self, i: Index) -> bool {
        i.0 != HEADER && self.nodes.get(i.0).is_some_and(|n| n.entry.is_some())
    }

    pub(crate) fn get(&self, i: Index) -> Option<(&K, &V)> {
        if !self.live(i) {
            return None;
        }
        self.nodes[i.0].entry.as_ref().map(|(k, v)| (k, v))
    }

    /// The node's key, changed in place without reordering: Boost's
    /// `const_cast<key_type&>(it->first)`, which keeps the map's order its own responsibility.
    pub(crate) fn key_mut(&mut self, i: Index) -> Option<&mut K> {
        if !self.live(i) {
            return None;
        }
        self.nodes[i.0].entry.as_mut().map(|(k, _)| k)
    }

    fn minimum(&self, mut x: usize) -> usize {
        while self.n(x).left != NIL {
            x = self.n(x).left;
        }
        x
    }
    fn maximum(&self, mut x: usize) -> usize {
        while self.n(x).right != NIL {
            x = self.n(x).right;
        }
        x
    }

    /// `_Rb_tree_increment`.
    fn increment(&self, mut x: usize) -> usize {
        if self.n(x).right != NIL {
            x = self.n(x).right;
            while self.n(x).left != NIL {
                x = self.n(x).left;
            }
        } else {
            let mut y = self.n(x).parent;
            while x == self.n(y).right {
                x = y;
                y = self.n(y).parent;
            }
            if self.n(x).right != y {
                x = y;
            }
        }
        x
    }

    /// `_Rb_tree_decrement`.
    fn decrement(&self, mut x: usize) -> usize {
        if self.n(x).color == Color::Red
            && self.n(x).parent != NIL
            && self.n(self.n(x).parent).parent == x
        {
            x = self.n(x).right;
        } else if self.n(x).left != NIL {
            let mut y = self.n(x).left;
            while self.n(y).right != NIL {
                y = self.n(y).right;
            }
            x = y;
        } else {
            let mut y = self.n(x).parent;
            while x == self.n(y).left {
                x = y;
                y = self.n(y).parent;
            }
            x = y;
        }
        x
    }

    /// The next node in order, None past the last (`end()`).
    pub(crate) fn next(&self, i: Option<Index>) -> Option<Index> {
        let i = i?;
        if !self.live(i) {
            return None;
        }
        let x = self.increment(i.0);
        (x != HEADER).then_some(Index(x))
    }

    /// The previous node in order: from `end()` (None) the last node; None before the first.
    pub(crate) fn prev(&self, i: Option<Index>) -> Option<Index> {
        match i {
            None => self.last(),
            Some(i) => {
                if !self.live(i) || self.n(HEADER).left == i.0 {
                    return None;
                }
                let x = self.decrement(i.0);
                (x != HEADER).then_some(Index(x))
            }
        }
    }

    /// `lower_bound(k)`: the first node not less than `k`, None for `end()`.
    pub(crate) fn lower_bound(&self, k: &K) -> Option<Index> {
        let mut x = self.root();
        let mut y = HEADER;
        while x != NIL {
            if !self.less(self.key(x), k) {
                y = x;
                x = self.n(x).left;
            } else {
                x = self.n(x).right;
            }
        }
        (y != HEADER).then_some(Index(y))
    }

    /// `_M_get_insert_unique_pos`: Ok((x, p)) to insert under `p`, or Err(existing node).
    fn get_insert_unique_pos(&self, k: &K) -> Result<(usize, usize), usize> {
        let mut x = self.root();
        let mut y = HEADER;
        let mut comp = true;
        while x != NIL {
            y = x;
            comp = self.less(k, self.key(x));
            x = if comp {
                self.n(x).left
            } else {
                self.n(x).right
            };
        }
        let mut j = y;
        if comp {
            if j == self.n(HEADER).left {
                return Ok((x, y));
            }
            j = self.decrement(j);
        }
        if self.less(self.key(j), k) {
            return Ok((x, y));
        }
        Err(j)
    }

    /// `_M_get_insert_hint_unique_pos`.
    fn get_insert_hint_unique_pos(&self, pos: usize, k: &K) -> Result<(usize, usize), usize> {
        if pos == HEADER {
            let rightmost = self.n(HEADER).right;
            if self.len > 0 && self.less(self.key(rightmost), k) {
                return Ok((NIL, rightmost));
            }
            return self.get_insert_unique_pos(k);
        }
        if self.less(k, self.key(pos)) {
            // First, try before...
            if pos == self.n(HEADER).left {
                return Ok((self.n(HEADER).left, self.n(HEADER).left));
            }
            let before = self.decrement(pos);
            if self.less(self.key(before), k) {
                if self.n(before).right == NIL {
                    return Ok((NIL, before));
                }
                return Ok((pos, pos));
            }
            return self.get_insert_unique_pos(k);
        }
        if self.less(self.key(pos), k) {
            // ... then try after.
            if pos == self.n(HEADER).right {
                return Ok((NIL, self.n(HEADER).right));
            }
            let after = self.increment(pos);
            if self.less(k, self.key(after)) {
                if self.n(pos).right == NIL {
                    return Ok((NIL, pos));
                }
                return Ok((after, after));
            }
            return self.get_insert_unique_pos(k);
        }
        // Equivalent keys.
        Err(pos)
    }

    /// `_M_insert_(x, p, v)`: the new node, rebalanced in.
    fn insert_at(&mut self, x: usize, p: usize, k: K, v: V) -> usize {
        let insert_left = x != NIL || p == HEADER || self.less(&k, self.key(p));
        let z = self.nodes.len();
        self.nodes.push(Node {
            parent: NIL,
            left: NIL,
            right: NIL,
            color: Color::Red,
            entry: Some((k, v)),
        });
        self.insert_and_rebalance(insert_left, z, p);
        self.len += 1;
        z
    }

    /// `insert(v)`: the new node, or the existing node with an equivalent key (unchanged).
    #[allow(dead_code)] // Boost inserts the beach line with hints only
    pub(crate) fn insert(&mut self, k: K, v: V) -> Index {
        match self.get_insert_unique_pos(&k) {
            Ok((x, p)) => Index(self.insert_at(x, p, k, v)),
            Err(existing) => Index(existing),
        }
    }

    /// `insert(hint, v)`, the hint a node or None for `end()`: the new node, or the existing
    /// node with an equivalent key (unchanged).
    pub(crate) fn insert_with_hint(&mut self, hint: Option<Index>, k: K, v: V) -> Index {
        let pos = match hint {
            Some(i) if self.live(i) => i.0,
            _ => HEADER,
        };
        match self.get_insert_hint_unique_pos(pos, &k) {
            Ok((x, p)) => Index(self.insert_at(x, p, k, v)),
            Err(existing) => Index(existing),
        }
    }

    /// `erase(it)`.
    pub(crate) fn erase(&mut self, i: Index) -> bool {
        if !self.live(i) {
            return false;
        }
        let y = self.rebalance_for_erase(i.0);
        self.nodes[y].entry = None;
        self.nodes[y].parent = NIL;
        self.nodes[y].left = NIL;
        self.nodes[y].right = NIL;
        self.len -= 1;
        true
    }

    /// In-order entries.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &V)> + '_ {
        let mut x = self.n(HEADER).left;
        std::iter::from_fn(move || {
            if x == HEADER {
                return None;
            }
            let here = x;
            x = self.increment(x);
            self.nodes[here].entry.as_ref().map(|(k, v)| (k, v))
        })
    }

    // ---- tree.cc ----

    fn set_root(&mut self, r: usize) {
        self.n_mut(HEADER).parent = r;
    }

    /// `local_Rb_tree_rotate_left`.
    fn rotate_left(&mut self, x: usize) {
        let y = self.n(x).right;
        let yl = self.n(y).left;
        self.n_mut(x).right = yl;
        if yl != NIL {
            self.n_mut(yl).parent = x;
        }
        let xp = self.n(x).parent;
        self.n_mut(y).parent = xp;
        if x == self.root() {
            self.set_root(y);
        } else if x == self.n(xp).left {
            self.n_mut(xp).left = y;
        } else {
            self.n_mut(xp).right = y;
        }
        self.n_mut(y).left = x;
        self.n_mut(x).parent = y;
    }

    /// `local_Rb_tree_rotate_right`.
    fn rotate_right(&mut self, x: usize) {
        let y = self.n(x).left;
        let yr = self.n(y).right;
        self.n_mut(x).left = yr;
        if yr != NIL {
            self.n_mut(yr).parent = x;
        }
        let xp = self.n(x).parent;
        self.n_mut(y).parent = xp;
        if x == self.root() {
            self.set_root(y);
        } else if x == self.n(xp).right {
            self.n_mut(xp).right = y;
        } else {
            self.n_mut(xp).left = y;
        }
        self.n_mut(y).right = x;
        self.n_mut(x).parent = y;
    }

    fn color(&self, x: usize) -> Color {
        self.n(x).color
    }

    /// `_Rb_tree_insert_and_rebalance`.
    fn insert_and_rebalance(&mut self, insert_left: bool, mut x: usize, p: usize) {
        {
            let node = self.n_mut(x);
            node.parent = p;
            node.left = NIL;
            node.right = NIL;
            node.color = Color::Red;
        }
        // Insert. Make the new node a child of its parent and maintain root, leftmost and
        // rightmost. N.B. the first node is always inserted left.
        if insert_left {
            self.n_mut(p).left = x; // also makes leftmost = x when p == header
            if p == HEADER {
                self.n_mut(HEADER).parent = x;
                self.n_mut(HEADER).right = x;
            } else if p == self.n(HEADER).left {
                self.n_mut(HEADER).left = x; // maintain leftmost pointing to min node
            }
        } else {
            self.n_mut(p).right = x;
            if p == self.n(HEADER).right {
                self.n_mut(HEADER).right = x; // maintain rightmost pointing to max node
            }
        }
        // Rebalance.
        while x != self.root() && self.color(self.n(x).parent) == Color::Red {
            let xp = self.n(x).parent;
            let xpp = self.n(xp).parent;
            if xp == self.n(xpp).left {
                let y = self.n(xpp).right;
                if y != NIL && self.color(y) == Color::Red {
                    self.n_mut(xp).color = Color::Black;
                    self.n_mut(y).color = Color::Black;
                    self.n_mut(xpp).color = Color::Red;
                    x = xpp;
                } else {
                    if x == self.n(xp).right {
                        x = xp;
                        self.rotate_left(x);
                    }
                    let xp = self.n(x).parent;
                    self.n_mut(xp).color = Color::Black;
                    self.n_mut(xpp).color = Color::Red;
                    self.rotate_right(xpp);
                }
            } else {
                let y = self.n(xpp).left;
                if y != NIL && self.color(y) == Color::Red {
                    self.n_mut(xp).color = Color::Black;
                    self.n_mut(y).color = Color::Black;
                    self.n_mut(xpp).color = Color::Red;
                    x = xpp;
                } else {
                    if x == self.n(xp).left {
                        x = xp;
                        self.rotate_right(x);
                    }
                    let xp = self.n(x).parent;
                    self.n_mut(xp).color = Color::Black;
                    self.n_mut(xpp).color = Color::Red;
                    self.rotate_left(xpp);
                }
            }
        }
        let root = self.root();
        self.n_mut(root).color = Color::Black;
    }

    /// `_Rb_tree_rebalance_for_erase`: unlinks `z` and returns it.
    fn rebalance_for_erase(&mut self, z: usize) -> usize {
        let mut y = z;
        let mut x;
        let mut x_parent;
        if self.n(y).left == NIL {
            // z has at most one non-null child. y == z.
            x = self.n(y).right; // x might be null.
        } else if self.n(y).right == NIL {
            // z has exactly one non-null child. y == z.
            x = self.n(y).left; // x is not null.
        } else {
            // z has two non-null children. Set y to z's successor. x might be null.
            y = self.n(y).right;
            while self.n(y).left != NIL {
                y = self.n(y).left;
            }
            x = self.n(y).right;
        }
        if y != z {
            // Relink y in place of z. y is z's successor.
            let zl = self.n(z).left;
            self.n_mut(zl).parent = y;
            self.n_mut(y).left = zl;
            if y != self.n(z).right {
                x_parent = self.n(y).parent;
                if x != NIL {
                    self.n_mut(x).parent = self.n(y).parent;
                }
                let yp = self.n(y).parent;
                self.n_mut(yp).left = x; // y must be a child of left
                let zr = self.n(z).right;
                self.n_mut(y).right = zr;
                self.n_mut(zr).parent = y;
            } else {
                x_parent = y;
            }
            let zp = self.n(z).parent;
            if self.root() == z {
                self.set_root(y);
            } else if self.n(zp).left == z {
                self.n_mut(zp).left = y;
            } else {
                self.n_mut(zp).right = y;
            }
            self.n_mut(y).parent = zp;
            let (yc, zc) = (self.n(y).color, self.n(z).color);
            self.n_mut(y).color = zc;
            self.n_mut(z).color = yc;
            y = z;
            // y now points to the node actually deleted.
        } else {
            // y == z
            x_parent = self.n(y).parent;
            if x != NIL {
                self.n_mut(x).parent = self.n(y).parent;
            }
            let zp = self.n(z).parent;
            if self.root() == z {
                self.set_root(x);
            } else if self.n(zp).left == z {
                self.n_mut(zp).left = x;
            } else {
                self.n_mut(zp).right = x;
            }
            if self.n(HEADER).left == z {
                if self.n(z).right == NIL {
                    // z's left must be null also; makes leftmost == header if z == root
                    self.n_mut(HEADER).left = self.n(z).parent;
                } else {
                    let m = self.minimum(x);
                    self.n_mut(HEADER).left = m;
                }
            }
            if self.n(HEADER).right == z {
                if self.n(z).left == NIL {
                    // z's right must be null also; makes rightmost == header if z == root
                    self.n_mut(HEADER).right = self.n(z).parent;
                } else {
                    // x == z's left
                    let m = self.maximum(x);
                    self.n_mut(HEADER).right = m;
                }
            }
        }
        if self.n(y).color != Color::Red {
            while x != self.root() && (x == NIL || self.color(x) == Color::Black) {
                if x == self.n(x_parent).left {
                    let mut w = self.n(x_parent).right;
                    if self.color(w) == Color::Red {
                        self.n_mut(w).color = Color::Black;
                        self.n_mut(x_parent).color = Color::Red;
                        self.rotate_left(x_parent);
                        w = self.n(x_parent).right;
                    }
                    let wl = self.n(w).left;
                    let wr = self.n(w).right;
                    if (wl == NIL || self.color(wl) == Color::Black)
                        && (wr == NIL || self.color(wr) == Color::Black)
                    {
                        self.n_mut(w).color = Color::Red;
                        x = x_parent;
                        x_parent = self.n(x_parent).parent;
                    } else {
                        if wr == NIL || self.color(wr) == Color::Black {
                            self.n_mut(wl).color = Color::Black;
                            self.n_mut(w).color = Color::Red;
                            self.rotate_right(w);
                            w = self.n(x_parent).right;
                        }
                        self.n_mut(w).color = self.n(x_parent).color;
                        self.n_mut(x_parent).color = Color::Black;
                        let wr = self.n(w).right;
                        if wr != NIL {
                            self.n_mut(wr).color = Color::Black;
                        }
                        self.rotate_left(x_parent);
                        break;
                    }
                } else {
                    // Same as above, with right <-> left.
                    let mut w = self.n(x_parent).left;
                    if self.color(w) == Color::Red {
                        self.n_mut(w).color = Color::Black;
                        self.n_mut(x_parent).color = Color::Red;
                        self.rotate_right(x_parent);
                        w = self.n(x_parent).left;
                    }
                    let wl = self.n(w).left;
                    let wr = self.n(w).right;
                    if (wr == NIL || self.color(wr) == Color::Black)
                        && (wl == NIL || self.color(wl) == Color::Black)
                    {
                        self.n_mut(w).color = Color::Red;
                        x = x_parent;
                        x_parent = self.n(x_parent).parent;
                    } else {
                        if wl == NIL || self.color(wl) == Color::Black {
                            self.n_mut(wr).color = Color::Black;
                            self.n_mut(w).color = Color::Red;
                            self.rotate_left(w);
                            w = self.n(x_parent).left;
                        }
                        self.n_mut(w).color = self.n(x_parent).color;
                        self.n_mut(x_parent).color = Color::Black;
                        let wl = self.n(w).left;
                        if wl != NIL {
                            self.n_mut(wl).color = Color::Black;
                        }
                        self.rotate_right(x_parent);
                        break;
                    }
                }
            }
            if x != NIL {
                self.n_mut(x).color = Color::Black;
            }
        }
        y
    }
}

#[cfg(test)]
impl KeyLess for i32 {
    fn key_less(&self, other: &Self) -> bool {
        self < other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl KeyLess for i64 {
        fn key_less(&self, other: &Self) -> bool {
            self < other
        }
    }

    /// Black height of every path is equal, no red node has a red child, links are mutual, and
    /// the header's leftmost and rightmost are the extremes.
    fn check(m: &RbMap<i64, ()>) {
        fn walk(m: &RbMap<i64, ()>, x: usize, parent: usize) -> usize {
            if x == NIL {
                return 1;
            }
            assert_eq!(m.n(x).parent, parent);
            if m.n(x).color == Color::Red {
                for c in [m.n(x).left, m.n(x).right] {
                    assert!(c == NIL || m.n(c).color == Color::Black);
                }
            }
            let l = walk(m, m.n(x).left, x);
            let r = walk(m, m.n(x).right, x);
            assert_eq!(l, r);
            l + usize::from(m.n(x).color == Color::Black)
        }
        let root = m.root();
        if root != NIL {
            assert_eq!(m.n(root).color, Color::Black);
            let _ = walk(m, root, HEADER);
            assert_eq!(m.n(HEADER).left, m.minimum(root));
            assert_eq!(m.n(HEADER).right, m.maximum(root));
        }
        let keys: Vec<i64> = m.iter().map(|(k, _)| *k).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(keys.len(), m.len());
    }

    #[test]
    fn inserts_hints_lower_bound_and_erase_keep_a_red_black_tree() {
        let mut m = RbMap::<i64, ()>::default();
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut rand = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut live: Vec<Index> = Vec::new();
        for step in 0..4000 {
            let r = rand();
            if live.is_empty() || r % 3 != 0 {
                let k = (rand() % 1000) as i64;
                let hint = match r % 4 {
                    0 => None,
                    _ => live.get((r as usize / 4) % live.len().max(1)).copied(),
                };
                let i = if r % 5 == 0 {
                    m.insert(k, ())
                } else {
                    m.insert_with_hint(hint, k, ())
                };
                assert_eq!(*m.get(i).unwrap().0, k);
                if !live.contains(&i) {
                    live.push(i);
                }
            } else {
                let at = (r as usize / 3) % live.len();
                let i = live.swap_remove(at);
                assert!(m.erase(i));
                assert!(m.get(i).is_none());
            }
            if step % 50 == 0 {
                check(&m);
                // lower_bound agrees with the sorted keys.
                let keys: Vec<i64> = m.iter().map(|(k, _)| *k).collect();
                for probe in [-1, 0, 250, 500, 999, 1000] {
                    let expect = keys.iter().find(|&&k| k >= probe).copied();
                    assert_eq!(m.lower_bound(&probe).map(|i| *m.get(i).unwrap().0), expect);
                }
                // next/prev walk the order both ways.
                let mut fwd = Vec::new();
                let mut at = m.first();
                while let Some(i) = at {
                    fwd.push(*m.get(i).unwrap().0);
                    at = m.next(at);
                }
                assert_eq!(fwd, keys);
                let mut back = Vec::new();
                let mut at = m.prev(None);
                while let Some(i) = at {
                    back.push(*m.get(i).unwrap().0);
                    at = m.prev(at);
                }
                back.reverse();
                assert_eq!(back, keys);
            }
        }
        check(&m);
    }
}
