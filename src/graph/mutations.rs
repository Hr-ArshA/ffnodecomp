//! Interactive edge/port lifecycle. Kept out of `node.rs` so the data model
//! stays small and the mutation rules have one obvious home.

use std::fmt;

use super::node::{compatible, Graph, Link, NodeId, PortKind};

/// Edges are addressed by their index in `Graph::links`. Any deletion
/// renumbers later edges, so callers must not hold an id across a mutation.
pub type EdgeId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortId {
    pub node: NodeId,
    pub port: usize,
    pub input: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    MissingNode(NodeId),
    MissingPort(PortId),
    NotAnOutput(PortId),
    NotAnInput(PortId),
    TypeMismatch { from: PortKind, to: PortKind },
    CyclicDependency,
    MissingEdge(EdgeId),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::MissingNode(id) => write!(f, "node #{id} does not exist"),
            GraphError::MissingPort(port) => write!(
                f,
                "{} port {} on node #{} does not exist",
                if port.input { "input" } else { "output" },
                port.port,
                port.node
            ),
            GraphError::NotAnOutput(port) => {
                write!(
                    f,
                    "port {} on node #{} is not an output",
                    port.port, port.node
                )
            }
            GraphError::NotAnInput(port) => {
                write!(
                    f,
                    "port {} on node #{} is not an input",
                    port.port, port.node
                )
            }
            GraphError::TypeMismatch { from, to } => {
                write!(f, "cannot connect {from:?} to {to:?}")
            }
            GraphError::CyclicDependency => write!(f, "connection would create a cycle"),
            GraphError::MissingEdge(id) => write!(f, "edge {id} does not exist"),
        }
    }
}

impl std::error::Error for GraphError {}

impl Graph {
    pub fn disconnect_edge(&mut self, edge: EdgeId) -> Result<(), GraphError> {
        if edge >= self.links.len() {
            return Err(GraphError::MissingEdge(edge));
        }
        self.links.remove(edge);
        Ok(())
    }

    /// Removes every link attached to the port. Outputs can fan out, so this
    /// may remove several links; an input removes at most one.
    #[allow(dead_code)]
    pub fn disconnect_port(&mut self, port: PortId) -> Result<(), GraphError> {
        let node = self
            .node(port.node)
            .ok_or(GraphError::MissingNode(port.node))?;
        let specs = if port.input {
            node.kind.inputs()
        } else {
            node.kind.outputs()
        };
        if specs.get(port.port).is_none() {
            return Err(GraphError::MissingPort(port));
        }

        if port.input {
            self.links
                .retain(|l| !(l.to == port.node && l.to_port == port.port));
        } else {
            self.links
                .retain(|l| !(l.from == port.node && l.from_port == port.port));
        }
        Ok(())
    }

    /// Validates types and cycles, then connects. An occupied input is
    /// replaced, preserving the single-input invariant.
    pub fn connect_ports(&mut self, from: PortId, to: PortId) -> Result<EdgeId, GraphError> {
        if from.input {
            return Err(GraphError::NotAnOutput(from));
        }
        if !to.input {
            return Err(GraphError::NotAnInput(to));
        }

        let source = self
            .node(from.node)
            .ok_or(GraphError::MissingNode(from.node))?;
        let target = self.node(to.node).ok_or(GraphError::MissingNode(to.node))?;

        let out = source
            .kind
            .outputs()
            .get(from.port)
            .ok_or(GraphError::MissingPort(from))?
            .kind;
        let input = target
            .kind
            .inputs()
            .get(to.port)
            .ok_or(GraphError::MissingPort(to))?
            .kind;

        if !compatible(out, input) {
            return Err(GraphError::TypeMismatch {
                from: out,
                to: input,
            });
        }
        if self.would_cycle(from.node, to.node) {
            return Err(GraphError::CyclicDependency);
        }

        self.links
            .retain(|l| !(l.to == to.node && l.to_port == to.port));
        self.links.push(Link {
            from: from.node,
            from_port: from.port,
            to: to.node,
            to_port: to.port,
        });
        Ok(self.links.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::node::{Graph, NodeKind};

    fn port(node: NodeId, port: usize, input: bool) -> PortId {
        PortId { node, port, input }
    }

    fn three_nodes() -> (Graph, NodeId, NodeId, NodeId) {
        let mut graph = Graph::default();
        let color = graph.add_node(
            NodeKind::Color {
                brightness: 0.0,
                contrast: 1.0,
                saturation: 1.0,
            },
            [0.0, 0.0],
        );
        let transform = graph.add_node(
            NodeKind::Transform {
                width: 640,
                height: 360,
                x: 0,
                y: 0,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(
            NodeKind::Output {
                width: 640,
                height: 360,
            },
            [0.0, 0.0],
        );
        (graph, color, transform, out)
    }

    #[test]
    fn connecting_replaces_an_occupied_input() {
        let (mut graph, color, transform, out) = three_nodes();
        graph
            .connect_ports(port(color, 0, false), port(transform, 0, true))
            .unwrap();
        graph
            .connect_ports(port(color, 0, false), port(transform, 0, true))
            .unwrap();
        assert_eq!(graph.links.len(), 1);

        // A second source replaces the first, never appends.
        graph
            .connect_ports(port(transform, 0, false), port(out, 0, true))
            .unwrap();
        graph
            .connect_ports(port(color, 0, false), port(out, 0, true))
            .unwrap();
        assert_eq!(graph.links.len(), 2);
        assert_eq!(graph.links[1].from, color);
    }

    #[test]
    fn disconnect_edge_and_port_remove_only_their_links() {
        let (mut graph, color, transform, out) = three_nodes();
        let _first = graph
            .connect_ports(port(color, 0, false), port(transform, 0, true))
            .unwrap();
        let second = graph
            .connect_ports(port(transform, 0, false), port(out, 0, true))
            .unwrap();

        graph.disconnect_edge(second).unwrap();
        assert_eq!(graph.links.len(), 1);
        assert!(
            graph.disconnect_edge(second).is_err(),
            "edge id is out of bounds"
        );

        graph.disconnect_port(port(color, 0, false)).unwrap();
        assert!(graph.links.is_empty());
    }

    #[test]
    fn cycles_and_direction_errors_are_rejected() {
        let (mut graph, color, transform, out) = three_nodes();
        graph
            .connect_ports(port(color, 0, false), port(transform, 0, true))
            .unwrap();
        assert_eq!(
            graph.connect_ports(port(transform, 0, false), port(color, 0, true)),
            Err(GraphError::CyclicDependency)
        );
        assert_eq!(
            graph.connect_ports(port(transform, 0, true), port(color, 0, true)),
            Err(GraphError::NotAnOutput(port(transform, 0, true)))
        );
        assert_eq!(
            graph.connect_ports(port(color, 0, false), port(out, 0, false)),
            Err(GraphError::NotAnInput(port(out, 0, false)))
        );
    }
}
