//! High-density, zero-allocation Behavior Tree engine for autonomous NPCs.
//!
//! Executes hierarchical decision trees using contiguous fixed-capacity arrays
//! and integer stack pointers, guaranteeing zero heap allocations during runtime evaluation.

/// Execution status of a behavior tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtStatus {
    /// Node evaluation succeeded.
    Success,
    /// Node evaluation failed.
    Failure,
    /// Node is actively running across multiple ticks.
    Running,
}

/// Node definition within the behavior tree array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtNodeType {
    /// Executes child nodes in order until one fails; succeeds if all children succeed.
    /// Parameters: (first_child_index, child_count).
    Sequence(u16, u16),
    /// Executes child nodes in order until one succeeds; fails if all children fail.
    /// Parameters: (first_child_index, child_count).
    Selector(u16, u16),
    /// Inverts the result of its single child node.
    /// Parameter: child_index.
    Inverter(u16),
    /// Leaf action node evaluated via application callback.
    /// Parameter: action_id.
    Action(u16),
    /// Leaf condition node evaluated via application callback.
    /// Parameter: condition_id.
    Condition(u16),
}

/// Behavior tree node record stored in contiguous memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BtNode {
    /// The node type and execution parameters.
    pub node_type: BtNodeType,
    /// The last execution status of this node.
    pub status: BtStatus,
}

impl BtNode {
    /// Creates a new un-evaluated node.
    pub const fn new(node_type: BtNodeType) -> Self {
        Self {
            node_type,
            status: BtStatus::Failure,
        }
    }
}

/// Fixed-capacity behavior tree supporting up to 32 nodes with zero heap allocations.
#[derive(Debug, Clone)]
pub struct BehaviorTree {
    nodes: [BtNode; 32],
    count: usize,
    root_index: u16,
}

impl Default for BehaviorTree {
    fn default() -> Self {
        Self {
            nodes: [BtNode::new(BtNodeType::Action(0)); 32],
            count: 0,
            root_index: 0,
        }
    }
}

impl BehaviorTree {
    /// Creates an empty behavior tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a node to the behavior tree and returns its index.
    pub fn add_node(&mut self, node_type: BtNodeType) -> Option<u16> {
        if self.count >= self.nodes.len() {
            return None;
        }
        let index = self.count as u16;
        self.nodes[self.count] = BtNode::new(node_type);
        self.count += 1;
        Some(index)
    }

    /// Sets the root node index of the tree.
    pub fn set_root(&mut self, root_index: u16) {
        self.root_index = root_index;
    }

    /// Returns the total node count.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Checks if the tree has zero nodes.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Evaluates the tree starting from the root node.
    ///
    /// Evaluator closures handle leaf conditions and actions with zero dynamic allocations.
    pub fn tick<FAction, FCondition>(
        &mut self,
        mut eval_action: FAction,
        mut eval_condition: FCondition,
    ) -> BtStatus
    where
        FAction: FnMut(u16) -> BtStatus,
        FCondition: FnMut(u16) -> bool,
    {
        if self.count == 0 || (self.root_index as usize) >= self.count {
            return BtStatus::Failure;
        }

        self.eval_node(self.root_index, &mut eval_action, &mut eval_condition)
    }

    fn eval_node<FAction, FCondition>(
        &mut self,
        node_idx: u16,
        eval_action: &mut FAction,
        eval_condition: &mut FCondition,
    ) -> BtStatus
    where
        FAction: FnMut(u16) -> BtStatus,
        FCondition: FnMut(u16) -> bool,
    {
        let node_type = match self.nodes.get(node_idx as usize) {
            Some(n) => n.node_type,
            None => return BtStatus::Failure,
        };

        let status = match node_type {
            BtNodeType::Condition(cond_id) => {
                if eval_condition(cond_id) {
                    BtStatus::Success
                } else {
                    BtStatus::Failure
                }
            }
            BtNodeType::Action(action_id) => eval_action(action_id),
            BtNodeType::Inverter(child_idx) => {
                let child_status = self.eval_node(child_idx, eval_action, eval_condition);
                match child_status {
                    BtStatus::Success => BtStatus::Failure,
                    BtStatus::Failure => BtStatus::Success,
                    BtStatus::Running => BtStatus::Running,
                }
            }
            BtNodeType::Sequence(start_idx, count) => {
                let mut seq_status = BtStatus::Success;
                for i in 0..count {
                    let child_idx = start_idx + i;
                    let child_status = self.eval_node(child_idx, eval_action, eval_condition);
                    if child_status != BtStatus::Success {
                        seq_status = child_status;
                        break;
                    }
                }
                seq_status
            }
            BtNodeType::Selector(start_idx, count) => {
                let mut sel_status = BtStatus::Failure;
                for i in 0..count {
                    let child_idx = start_idx + i;
                    let child_status = self.eval_node(child_idx, eval_action, eval_condition);
                    if child_status != BtStatus::Failure {
                        sel_status = child_status;
                        break;
                    }
                }
                sel_status
            }
        };

        if let Some(n) = self.nodes.get_mut(node_idx as usize) {
            n.status = status;
        }

        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequence_success_when_all_children_succeed() {
        let mut tree = BehaviorTree::new();
        // Node 0: Sequence with 2 children starting at index 1
        let seq = tree.add_node(BtNodeType::Sequence(1, 2)).unwrap();
        // Node 1: Condition 1 (true)
        let _c1 = tree.add_node(BtNodeType::Condition(1)).unwrap();
        // Node 2: Action 1 (Success)
        let _a1 = tree.add_node(BtNodeType::Action(1)).unwrap();

        tree.set_root(seq);

        let status = tree.tick(|_act| BtStatus::Success, |_cond| true);
        assert_eq!(status, BtStatus::Success);
    }

    #[test]
    fn test_sequence_fails_on_first_failure() {
        let mut tree = BehaviorTree::new();
        let seq = tree.add_node(BtNodeType::Sequence(1, 2)).unwrap();
        let _c1 = tree.add_node(BtNodeType::Condition(1)).unwrap();
        let _a1 = tree.add_node(BtNodeType::Action(1)).unwrap();

        tree.set_root(seq);

        let mut action_called = false;
        let status = tree.tick(
            |_act| {
                action_called = true;
                BtStatus::Success
            },
            |_cond| false, // Condition fails!
        );

        assert_eq!(status, BtStatus::Failure);
        assert!(!action_called); // Short-circuit: Action was not evaluated
    }

    #[test]
    fn test_selector_fallback() {
        let mut tree = BehaviorTree::new();
        // Node 0: Selector with 2 children starting at index 1
        let sel = tree.add_node(BtNodeType::Selector(1, 2)).unwrap();
        // Node 1: Action 1 (Fails)
        let _a1 = tree.add_node(BtNodeType::Action(1)).unwrap();
        // Node 2: Action 2 (Succeeds)
        let _a2 = tree.add_node(BtNodeType::Action(2)).unwrap();

        tree.set_root(sel);

        let status = tree.tick(
            |act| {
                if act == 1 {
                    BtStatus::Failure
                } else {
                    BtStatus::Success
                }
            },
            |_cond| true,
        );

        assert_eq!(status, BtStatus::Success);
    }

    #[test]
    fn test_inverter_inverts_child() {
        let mut tree = BehaviorTree::new();
        let inv = tree.add_node(BtNodeType::Inverter(1)).unwrap();
        let _c1 = tree.add_node(BtNodeType::Condition(1)).unwrap();

        tree.set_root(inv);

        // Child condition returns true -> Inverter should return Failure
        let status = tree.tick(|_act| BtStatus::Success, |_cond| true);
        assert_eq!(status, BtStatus::Failure);

        // Child condition returns false -> Inverter should return Success
        let status2 = tree.tick(|_act| BtStatus::Success, |_cond| false);
        assert_eq!(status2, BtStatus::Success);
    }
}
