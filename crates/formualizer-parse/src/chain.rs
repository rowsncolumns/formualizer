//! Walking binary-operator chains without recursing down their spine.
//!
//! Every infix operator parses left-associatively, so `A1+A2+…+An` is a tree whose LEFT edge is
//! n levels deep. Excel accepts formulas of up to 8,192 characters, so `=1+1+…+1` is a
//! 4,096-level spine. A pass that recurses into `left` puts every level on the call stack, and in
//! wasm that exhausts the host thread's native stack long before 4,096 levels (the trap then
//! poisons the whole module — Athena-Intel/demo-app-monorepo#49969 FX-02).
//!
//! The helpers here let a pass visit a chain's operands in evaluation order with a loop: the
//! bottom (leftmost) operand first, then each binary node's right operand from the deepest node
//! up. A pass still recurses into those operands, but their depth is bounded by the formula's
//! visible nesting (brackets, function calls, the handful of precedence levels), not by its length.

use crate::LiteralValue;
use crate::parser::{ASTNode, ASTNodeType};

impl ASTNode {
    /// The leftmost operand of the binary chain rooted at `self`, and the chain's binary nodes,
    /// deepest first. A node that is not a binary operation is its own bottom with an empty spine.
    ///
    /// For `((a+b)-c)*d` this is `(a, [a+b, (a+b)-c, ((a+b)-c)*d])`: fold `a`, then each node's
    /// right operand in order, to evaluate the chain left to right.
    pub fn binary_chain(&self) -> (&ASTNode, Vec<&ASTNode>) {
        let mut spine = Vec::new();
        let mut cur = self;
        while let ASTNodeType::BinaryOp { left, .. } = &cur.node_type {
            spine.push(cur);
            cur = left;
        }
        spine.reverse();
        (cur, spine)
    }

    /// The operator and right operand of a binary node. Panics on any other node; meant for the
    /// nodes [`ASTNode::binary_chain`] returns.
    pub fn binary_parts(&self) -> (&str, &ASTNode) {
        match &self.node_type {
            ASTNodeType::BinaryOp { op, right, .. } => (op.as_str(), right),
            _ => panic!("binary_parts called on a non-binary node"),
        }
    }

    /// Owned variant of [`ASTNode::binary_chain`]: detaches the chain into its bottom operand and
    /// its binary nodes (deepest first), each with its `left` operand replaced by an empty
    /// placeholder. [`ASTNode::from_binary_chain`] reassembles it.
    pub fn into_binary_chain(self) -> (ASTNode, Vec<ASTNode>) {
        let mut spine = Vec::new();
        let mut cur = self;
        while matches!(cur.node_type, ASTNodeType::BinaryOp { .. }) {
            let ASTNodeType::BinaryOp { left, .. } = &mut cur.node_type else {
                unreachable!("checked above");
            };
            let next = std::mem::replace(&mut **left, placeholder());
            spine.push(cur);
            cur = next;
        }
        spine.reverse();
        (cur, spine)
    }

    /// Reassembles a chain detached by [`ASTNode::into_binary_chain`]: each binary node in
    /// `spine` (deepest first) takes the chain built so far as its left operand.
    pub fn from_binary_chain(bottom: ASTNode, spine: Vec<ASTNode>) -> ASTNode {
        let mut acc = bottom;
        for mut node in spine {
            if let ASTNodeType::BinaryOp { left, .. } = &mut node.node_type {
                **left = acc;
            }
            acc = node;
        }
        acc
    }

    /// Calls `f` on every operand of the binary chain rooted at `self`, in evaluation order (the
    /// bottom operand, then each right operand from the deepest node up), stopping at the first
    /// error. The chain is walked with a loop, so `f` may recurse into the operands. A node that is
    /// not a binary operation is passed to `f` itself.
    pub fn try_for_each_chain_operand_mut<E>(
        &mut self,
        mut f: impl FnMut(&mut ASTNode) -> Result<(), E>,
    ) -> Result<(), E> {
        if !matches!(self.node_type, ASTNodeType::BinaryOp { .. }) {
            return f(self);
        }
        let owned = std::mem::replace(self, placeholder());
        let (mut bottom, mut spine) = owned.into_binary_chain();
        let mut result = f(&mut bottom);
        if result.is_ok() {
            for node in &mut spine {
                if let ASTNodeType::BinaryOp { right, .. } = &mut node.node_type {
                    result = f(right);
                    if result.is_err() {
                        break;
                    }
                }
            }
        }
        *self = ASTNode::from_binary_chain(bottom, spine);
        result
    }

    /// Infallible [`ASTNode::try_for_each_chain_operand_mut`].
    pub fn for_each_chain_operand_mut(&mut self, mut f: impl FnMut(&mut ASTNode)) {
        let _ = self.try_for_each_chain_operand_mut::<std::convert::Infallible>(|node| {
            f(node);
            Ok(())
        });
    }

    /// Calls `f` on every operand of the binary chain rooted at `self`, in evaluation order. See
    /// [`ASTNode::try_for_each_chain_operand_mut`].
    pub fn for_each_chain_operand<'a>(&'a self, mut f: impl FnMut(&'a ASTNode)) {
        let (bottom, spine) = self.binary_chain();
        f(bottom);
        for node in spine {
            f(node.binary_parts().1);
        }
    }

    /// Fallible [`ASTNode::for_each_chain_operand`].
    pub fn try_for_each_chain_operand<'a, E>(
        &'a self,
        mut f: impl FnMut(&'a ASTNode) -> Result<(), E>,
    ) -> Result<(), E> {
        let (bottom, spine) = self.binary_chain();
        f(bottom)?;
        for node in spine {
            f(node.binary_parts().1)?;
        }
        Ok(())
    }

    /// Folds the binary chain rooted at `self` left to right: `operand` maps each operand (in
    /// evaluation order) and `combine(node, left, right)` merges a binary node's folded left side
    /// with its mapped right operand.
    pub fn fold_binary_chain<'a, T>(
        &'a self,
        mut operand: impl FnMut(&'a ASTNode) -> T,
        mut combine: impl FnMut(&'a ASTNode, T, T) -> T,
    ) -> T {
        let (bottom, spine) = self.binary_chain();
        let mut acc = operand(bottom);
        for node in spine {
            let right = operand(node.binary_parts().1);
            acc = combine(node, acc, right);
        }
        acc
    }

    /// Rebuilds the binary chain rooted at `self` with every operand replaced by `f(operand)`
    /// (called in evaluation order), keeping each binary node's operator, token and volatility.
    pub fn map_binary_chain(&self, mut f: impl FnMut(&ASTNode) -> ASTNode) -> ASTNode {
        let (bottom, spine) = self.binary_chain();
        let mut acc = f(bottom);
        for node in spine {
            let right = f(node.binary_parts().1);
            acc = node.with_binary_operands(acc, right);
        }
        acc
    }

    /// Like [`ASTNode::map_binary_chain`], but `f` returns `None` for an operand it leaves alone;
    /// `None` overall when every operand was left alone. Unchanged operands are cloned only when
    /// some other operand changed.
    pub fn map_binary_chain_if_changed(
        &self,
        mut f: impl FnMut(&ASTNode) -> Option<ASTNode>,
    ) -> Option<ASTNode> {
        let (bottom, spine) = self.binary_chain();
        let bottom_adj = f(bottom);
        let rights: Vec<Option<ASTNode>> =
            spine.iter().map(|node| f(node.binary_parts().1)).collect();
        if bottom_adj.is_none() && rights.iter().all(Option::is_none) {
            return None;
        }
        let mut acc = bottom_adj.unwrap_or_else(|| bottom.clone());
        for (node, right) in spine.into_iter().zip(rights) {
            let right = right.unwrap_or_else(|| node.binary_parts().1.clone());
            acc = node.with_binary_operands(acc, right);
        }
        Some(acc)
    }

    /// A copy of this binary node (operator, token, volatility) over the given operands.
    fn with_binary_operands(&self, left: ASTNode, right: ASTNode) -> ASTNode {
        ASTNode {
            node_type: ASTNodeType::BinaryOp {
                op: self.binary_parts().0.to_string(),
                left: Box::new(left),
                right: Box::new(right),
            },
            source_token: self.source_token.clone(),
            contains_volatile: self.contains_volatile,
        }
    }

    /// Whether `f` holds for any operand of the binary chain rooted at `self` (evaluation order,
    /// short-circuiting).
    pub fn any_chain_operand<'a>(&'a self, mut f: impl FnMut(&'a ASTNode) -> bool) -> bool {
        let (bottom, spine) = self.binary_chain();
        f(bottom) || spine.into_iter().any(|node| f(node.binary_parts().1))
    }
}

/// Cloning walks binary chains with a loop: the derived clone recursed once per level, which is as
/// deep as a chain is long.
impl Clone for ASTNode {
    fn clone(&self) -> Self {
        if !matches!(self.node_type, ASTNodeType::BinaryOp { .. }) {
            return ASTNode {
                node_type: self.node_type.clone(),
                source_token: self.source_token.clone(),
                contains_volatile: self.contains_volatile,
            };
        }
        let (bottom, spine) = self.binary_chain();
        let mut acc = bottom.clone();
        for node in spine {
            let (op, right) = node.binary_parts();
            acc = ASTNode {
                node_type: ASTNodeType::BinaryOp {
                    op: op.to_string(),
                    left: Box::new(acc),
                    right: Box::new(right.clone()),
                },
                source_token: node.source_token.clone(),
                contains_volatile: node.contains_volatile,
            };
        }
        acc
    }
}

/// Structural equality, walking binary chains with a loop (see [`Clone`] above).
impl PartialEq for ASTNode {
    fn eq(&self, other: &Self) -> bool {
        let both_binary = matches!(self.node_type, ASTNodeType::BinaryOp { .. })
            && matches!(other.node_type, ASTNodeType::BinaryOp { .. });
        if !both_binary {
            return self.node_type == other.node_type
                && self.source_token == other.source_token
                && self.contains_volatile == other.contains_volatile;
        }
        let (a_bottom, a_spine) = self.binary_chain();
        let (b_bottom, b_spine) = other.binary_chain();
        a_spine.len() == b_spine.len()
            && a_spine.iter().zip(&b_spine).all(|(a, b)| {
                let (a_op, a_right) = a.binary_parts();
                let (b_op, b_right) = b.binary_parts();
                a_op == b_op
                    && a.source_token == b.source_token
                    && a.contains_volatile == b.contains_volatile
                    && a_right == b_right
            })
            && a_bottom == b_bottom
    }
}

fn placeholder() -> ASTNode {
    ASTNode::new(ASTNodeType::Literal(LiteralValue::Empty), None)
}

/// Dropping walks binary and unary chains with a loop: the derived drop glue recursed once per
/// level, which is as deep as a chain is long. Children that are themselves operators are moved
/// onto a heap stack and dropped one at a time; everything else drops as usual (its depth is
/// bounded by the formula's visible nesting).
impl Drop for ASTNode {
    fn drop(&mut self) {
        let mut stack = Vec::new();
        detach_operator_children(&mut self.node_type, &mut stack);
        while let Some(mut node) = stack.pop() {
            detach_operator_children(&mut node.node_type, &mut stack);
        }
    }
}

fn is_operator(node: &ASTNode) -> bool {
    matches!(
        node.node_type,
        ASTNodeType::BinaryOp { .. } | ASTNodeType::UnaryOp { .. }
    )
}

fn detach_operator_children(node_type: &mut ASTNodeType, stack: &mut Vec<ASTNode>) {
    match node_type {
        ASTNodeType::BinaryOp { left, right, .. } => {
            for child in [left, right] {
                if is_operator(child) {
                    stack.push(std::mem::replace(&mut **child, placeholder()));
                }
            }
        }
        ASTNodeType::UnaryOp { expr, .. } => {
            if is_operator(expr) {
                stack.push(std::mem::replace(&mut **expr, placeholder()));
            }
        }
        _ => {}
    }
}
