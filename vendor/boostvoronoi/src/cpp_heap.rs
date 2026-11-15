// SPDX-License-Identifier:BSL-1.0

// A priority queue that keeps the same heap, and so breaks ties between equal elements the same
// way, as libstdc++'s `std::priority_queue` over a `std::vector` (`push_heap`, `pop_heap`:
// `__push_heap`, `__adjust_heap`, `__pop_heap` in `stl_heap.h`), which C++ Boost.Polygon's
// voronoi builder uses for its circle events and its segment end points. Which of two equal
// events surfaces first decides which beach line node a circle event closes, so the reference
// implementation's diagram depends on this order.

/// `less(a, b)`: `a` has lower priority than `b` (std::priority_queue's `Compare`); the top
/// is an element no other is greater than.
pub(crate) struct CppPriorityQueue<T, F: Fn(&T, &T) -> bool> {
    c: Vec<T>,
    less: F,
}

impl<T, F: Fn(&T, &T) -> bool> CppPriorityQueue<T, F> {
    pub(crate) fn new(less: F) -> Self {
        Self {
            c: Vec::new(),
            less,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.c.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.c.is_empty()
    }

    pub(crate) fn top(&self) -> Option<&T> {
        self.c.first()
    }

    pub(crate) fn clear(&mut self) {
        self.c.clear();
    }

    /// `push(x)`: `c.push_back(x); push_heap(c.begin(), c.end(), comp)`.
    pub(crate) fn push(&mut self, x: T) {
        self.c.push(x);
        let hole = self.c.len() - 1;
        self.push_heap_hole(hole, 0);
    }

    /// `pop()`: `pop_heap(c.begin(), c.end(), comp); c.pop_back()`.
    pub(crate) fn pop(&mut self) -> Option<T> {
        let len = self.c.len();
        if len > 1 {
            // __pop_heap(first, last - 1, last - 1): the last element becomes `value`, the top
            // goes to the end, and the hole at the top is adjusted down for `value`.
            self.c.swap(0, len - 1);
            self.adjust_heap(0, len - 1);
        }
        self.c.pop()
    }

    /// `__push_heap(first, hole, top, value)`, `value` held at `hole`.
    fn push_heap_hole(&mut self, mut hole: usize, top: usize) {
        while hole > top {
            let parent = (hole - 1) / 2;
            if !(self.less)(&self.c[parent], &self.c[hole]) {
                break;
            }
            self.c.swap(parent, hole);
            hole = parent;
        }
    }

    /// `__adjust_heap(first, hole, len, value)`, `value` held at `hole` (the top) over the
    /// first `len` elements.
    fn adjust_heap(&mut self, mut hole: usize, len: usize) {
        let top = hole;
        let mut second = hole;
        while second < len.saturating_sub(1) / 2 {
            second = 2 * (second + 1);
            if (self.less)(&self.c[second], &self.c[second - 1]) {
                second -= 1;
            }
            self.c.swap(hole, second);
            hole = second;
        }
        if len & 1 == 0 && len >= 2 && second == (len - 2) / 2 {
            second = 2 * (second + 1);
            self.c.swap(hole, second - 1);
            hole = second - 1;
        }
        self.push_heap_hole(hole, top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pops always return a greatest element (the heap property under push and pop). The
    /// order among equal elements is checked against C++ by the diagram differential tests.
    #[test]
    fn pops_in_order_and_keeps_a_heap() {
        let mut q = CppPriorityQueue::new(|a: &(i32, i32), b: &(i32, i32)| a.0 < b.0);
        let mut state = 7u32;
        let mut all = Vec::new();
        for i in 0..500 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let k = ((state >> 16) % 20) as i32;
            q.push((k, i));
            all.push(k);
            if i % 7 == 3 {
                let top = q.pop().unwrap();
                let max = *all.iter().max().unwrap();
                assert_eq!(top.0, max);
                let at = all.iter().position(|&x| x == max).unwrap();
                let _ = all.swap_remove(at);
            }
        }
        while let Some(t) = q.pop() {
            let max = *all.iter().max().unwrap();
            assert_eq!(t.0, max);
            let at = all.iter().position(|&x| x == max).unwrap();
            let _ = all.swap_remove(at);
        }
        assert!(all.is_empty());
    }
}
