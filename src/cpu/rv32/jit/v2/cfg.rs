//! Control Flow Graph analysis

use std::collections::{HashMap, HashSet};
use super::types::{BasicBlock, ControlFlowStructure};

/// Control flow graph represented as adjacency list
pub type CfgGraph = HashMap<u32, HashSet<u32>>;

/// Build CFG from basic blocks
pub fn build_cfg(blocks: &[BasicBlock]) -> CfgGraph {
    let mut graph = CfgGraph::new();

    for block in blocks {
        let successors: HashSet<u32> = block.successors().into_iter().collect();
        graph.insert(block.addr, successors);
    }

    graph
}

/// Reverse all edges in a graph
fn reverse_graph(graph: &CfgGraph) -> CfgGraph {
    let mut rev = CfgGraph::new();

    // Ensure all nodes exist in reverse graph
    for &node in graph.keys() {
        rev.entry(node).or_default();
    }

    // Add reversed edges
    for (&from, tos) in graph {
        for &to in tos {
            rev.entry(to).or_default().insert(from);
        }
    }

    rev
}

/// Find strongly connected components using Kosaraju's algorithm
///
/// Returns SCCs in reverse topological order (leaves first)
pub fn find_sccs(graph: &CfgGraph) -> Vec<Vec<u32>> {
    // Phase 1: DFS to get finish order
    let mut visited = HashSet::new();
    let mut finish_order = Vec::new();

    fn dfs_finish(
        node: u32,
        graph: &CfgGraph,
        visited: &mut HashSet<u32>,
        finish_order: &mut Vec<u32>,
    ) {
        if visited.contains(&node) {
            return;
        }
        visited.insert(node);

        if let Some(successors) = graph.get(&node) {
            for &succ in successors {
                dfs_finish(succ, graph, visited, finish_order);
            }
        }

        finish_order.push(node);
    }

    for &node in graph.keys() {
        dfs_finish(node, graph, &mut visited, &mut finish_order);
    }

    // Phase 2: DFS on reverse graph in reverse finish order
    let rev_graph = reverse_graph(graph);
    let mut visited = HashSet::new();
    let mut sccs = Vec::new();

    fn dfs_collect(
        node: u32,
        rev_graph: &CfgGraph,
        visited: &mut HashSet<u32>,
        component: &mut Vec<u32>,
    ) {
        if visited.contains(&node) {
            return;
        }
        visited.insert(node);
        component.push(node);

        if let Some(predecessors) = rev_graph.get(&node) {
            for &pred in predecessors {
                dfs_collect(pred, rev_graph, visited, component);
            }
        }
    }

    for &node in finish_order.iter().rev() {
        if !visited.contains(&node) {
            let mut component = Vec::new();
            dfs_collect(node, &rev_graph, &mut visited, &mut component);
            if !component.is_empty() {
                sccs.push(component);
            }
        }
    }

    sccs
}

/// Convert SCCs to structured control flow
///
/// This is a simplified version of v86's loopify/blockify
pub fn structure_sccs(
    graph: &CfgGraph,
    sccs: &[Vec<u32>],
    entry_points: &[u32]
) -> Vec<ControlFlowStructure> {
    let mut result = Vec::new();

    // Add dispatcher if multiple entry points
    if entry_points.len() > 1 {
        result.push(ControlFlowStructure::Dispatcher(entry_points.to_vec()));
    }

    for scc in sccs {
        if scc.is_empty() {
            continue;
        }

        if scc.len() == 1 {
            let addr = scc[0];
            // Check for self-loop
            let is_self_loop = graph
                .get(&addr)
                .map_or(false, |succs| succs.contains(&addr));

            if is_self_loop {
                result.push(ControlFlowStructure::Loop(vec![
                    ControlFlowStructure::Block(addr),
                ]));
            } else {
                result.push(ControlFlowStructure::Block(addr));
            }
        } else {
            // Multi-block SCC = loop
            let inner: Vec<ControlFlowStructure> = scc
                .iter()
                .map(|&addr| ControlFlowStructure::Block(addr))
                .collect();
            result.push(ControlFlowStructure::Loop(inner));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scc_simple() {
        // Simple graph: A -> B -> C
        let mut graph = CfgGraph::new();
        graph.insert(1, [2].into_iter().collect());
        graph.insert(2, [3].into_iter().collect());
        graph.insert(3, HashSet::new());

        let sccs = find_sccs(&graph);
        // Each node is its own SCC (no cycles)
        assert_eq!(sccs.len(), 3);
    }

    #[test]
    fn test_scc_loop() {
        // Graph with loop: A -> B -> C -> A
        let mut graph = CfgGraph::new();
        graph.insert(1, [2].into_iter().collect());
        graph.insert(2, [3].into_iter().collect());
        graph.insert(3, [1].into_iter().collect());

        let sccs = find_sccs(&graph);
        // All nodes in one SCC
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 3);
    }
}
