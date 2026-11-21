//! Finds the first usable gap in stream order. Each leaf is keyed by the start
//! of the record to its right and stores the number of bytes available before it.
//! Empty gaps and the unbounded space after the last record need no entries.

#[derive(Default)]
pub(super) struct GapIndex(Option<Box<Node>>);

struct Node {
    // Any descendant key identifies the prefix shared by this subtree.
    key: u64,
    max: u64,
    branch: Option<(u64, Box<Node>, Box<Node>)>,
}

impl GapIndex {
    pub(super) fn set(&mut self, end: u64, capacity: u64) {
        let root = remove(self.0.take(), end);
        self.0 = if capacity == 0 {
            root
        } else {
            let leaf = Box::new(Node {
                key: end,
                max: capacity,
                branch: None,
            });
            Some(match root {
                Some(root) => insert(root, leaf),
                None => leaf,
            })
        };
    }

    pub(super) fn first_fit(&self, required: u64) -> Option<u64> {
        let mut node = self.0.as_deref().filter(|node| node.max >= required)?;
        while let Some((_, left, right)) = &node.branch {
            node = if left.max >= required { left } else { right };
        }
        Some(node.key - node.max)
    }
}

fn parent(bit: u64, left: Box<Node>, right: Box<Node>) -> Box<Node> {
    Box::new(Node {
        key: left.key,
        max: left.max.max(right.max),
        branch: Some((bit, left, right)),
    })
}

// Branch only on bits that distinguish keys. Every descent uses a lower bit,
// so lookup and updates visit at most 64 branches.
fn insert(mut node: Box<Node>, leaf: Box<Node>) -> Box<Node> {
    let difference = node.key ^ leaf.key;
    if let Some((bit, left, right)) = node.branch.take() {
        if difference & !(bit | (bit - 1)) == 0 {
            return if leaf.key & bit == 0 {
                parent(bit, insert(left, leaf), right)
            } else {
                parent(bit, left, insert(right, leaf))
            };
        }
        node.branch = Some((bit, left, right));
    }
    let bit = 1 << (63 - difference.leading_zeros());
    if leaf.key & bit == 0 {
        parent(bit, leaf, node)
    } else {
        parent(bit, node, leaf)
    }
}

fn remove(root: Option<Box<Node>>, key: u64) -> Option<Box<Node>> {
    let mut node = root?;
    match node.branch.take() {
        None => (node.key != key).then_some(node),
        Some((bit, left, right)) => {
            if key & bit == 0 {
                Some(match remove(Some(left), key) {
                    Some(left) => parent(bit, left, right),
                    None => right,
                })
            } else {
                Some(match remove(Some(right), key) {
                    Some(right) => parent(bit, left, right),
                    None => left,
                })
            }
        }
    }
}
