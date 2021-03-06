use chrono::prelude::*;
use petgraph::dot::{Config, Dot};
use petgraph::prelude::*;
use petgraph_graphml::GraphMl;
use std::fmt;
use std::fs;
use std::io;
use std::io::prelude::*;
use std::path::{Path, PathBuf};

use rand::Rng;
use std::collections::hash_map::DefaultHasher;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};

pub type Graph = petgraph::Graph<Node, (), petgraph::Undirected>;

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeType {
    Room,
    User,
    Server,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeType,
    pub id: u64,
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.kind {
            NodeType::Room => write!(f, "room_{}", self.id),
            NodeType::User => write!(f, "user_{}", self.id),
            NodeType::Server => write!(f, "server_{}", self.id),
        }
    }
}

// hack around the type signature of Dot::fmt which requires both node and edge data types to implement Display.
// But as I have no edge data, I want to use (), which does not implement Display, though.
// Convert to this type before using Dot::fmt. As I use the EdgeNoLabel option of Dot::fmt, unreachable! is enough.
struct NoEdgeData;
impl fmt::Display for NoEdgeData {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        unreachable!();
    }
}

fn hash_with_salt(builder: &dyn BuildHasher<Hasher = DefaultHasher>, x: &impl Hash, salt: u64) -> u64 {
    let mut hasher = builder.build_hasher();
    x.hash(&mut hasher);
    salt.hash(&mut hasher);
    hasher.finish()
}

pub fn read_graph<P: AsRef<Path>>(path: P) -> Result<Graph, serde_json::Error> {
    let file = fs::File::open(path).unwrap();
    let reader = io::BufReader::new(file);
    serde_json::from_reader(reader)
}

pub fn graph_dir() -> PathBuf {
    let local: DateTime<Local> = Local::now();
    let dir = PathBuf::from(format!(
        "data/graphs/graph_{}",
        local.format("%Y-%m-%dT%H-%M-%S")
    ));
    if !dir.exists() {
        fs::create_dir_all(&dir).unwrap();
    }
    dir
}

pub fn write_graph<P: AsRef<Path>>(graph: &Graph, dir: P) -> Result<(), serde_json::Error> {
    let path = dir.as_ref().join("graph.json");
    let file = fs::File::create(path).expect("Could not create graph file");
    let writer = io::BufWriter::new(file);
    serde_json::to_writer(writer, graph)
}

pub fn export_graph_to_graphml<P: AsRef<Path>>(graph: &Graph, dir: P) -> io::Result<()> {
    let graphml = GraphMl::new(&graph)
        .pretty_print(true)
        .export_node_weights_display();
    let file = fs::File::create(dir.as_ref().join("graph.graphml"))
        .expect("Could not create graph/graph.graphml file");
    let writer = io::BufWriter::new(file);
    graphml.to_writer(writer)
}

pub fn export_graph_to_dot<P: AsRef<Path>>(graph: &Graph, dir: P) -> io::Result<()> {
    let no_edge_data = graph.map(|_, node| node, |_, _| NoEdgeData);
    let exported_graph = Dot::with_config(&no_edge_data, &[Config::EdgeNoLabel]);
    let file = fs::File::create(dir.as_ref().join("graph.dot"))
        .expect("Could not create graph/graph.dot file");
    let mut buffer = io::BufWriter::new(file);
    write!(&mut buffer, "{}", exported_graph)
}

pub fn anonymize_graph(graph: Graph) -> Graph {
    let hash_key = RandomState::new();
    let mut rng = rand::thread_rng();
    let salt = rng.gen::<u64>();
    graph.map(
        |_, node| Node {
            kind: node.kind,
            id: hash_with_salt(&hash_key, &node.id, salt),
        },
        |_, _| (),
    )
}

fn is_wellformed_node(graph: &Graph, idx: NodeIndex) -> bool {
    let is_wellformed = match graph[idx].kind {
        NodeType::User => {
            // a user needs exactly one HS and be a member of at least one room.
            // This should be impossible, as we get the HS from the user id and find users through a room.
            graph
                .neighbors(idx)
                .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::Server)
                .count() == 1
                && graph
                    .neighbors(idx)
                    .any(|neighbor_idx| graph[neighbor_idx].kind == NodeType::Room)
        },
        NodeType::Room => {
            // A room needs at least one user and at least one server. Could be caused by ignore patterns.
            // As those disconnected rooms do nothing for the simulation an only dillute the results, I should remove them.
            graph
                .neighbors(idx)
                .any(|neighbor_idx| graph[neighbor_idx].kind == NodeType::User)
                && graph
                    .neighbors(idx)
                    .any(|neighbor_idx| graph[neighbor_idx].kind == NodeType::Server)
        },
        NodeType::Server => {
            // A server needs at least one user and at least one room.
            // This should be impossible, as we only can see servers through a user in a room.
            graph
                .neighbors(idx)
                .any(|neighbor_idx| graph[neighbor_idx].kind == NodeType::User)
                && graph
                    .neighbors(idx)
                    .any(|neighbor_idx| graph[neighbor_idx].kind == NodeType::Room)
        },
    };
    if !is_wellformed {
        eprintln!(
            "malformed node: {}. neighbors: {} users, {} rooms, {} servers.",
            graph[idx],
            graph
                .neighbors(idx)
                .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::User)
                .count(),
            graph
                .neighbors(idx)
                .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::Room)
                .count(),
            graph
                .neighbors(idx)
                .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::Server)
                .count(),
        );
    }
    is_wellformed
}

pub fn is_wellformed_graph(graph: &Graph) -> bool {
    graph
        .node_indices()
        .all(|idx| is_wellformed_node(graph, idx))
}

// returns map from server id to number of users and rooms
pub fn users_rooms_per_server_distribution(graph: &Graph) -> HashMap<u64, (usize, usize)> {
    graph
        .node_indices()
        .filter(|idx| graph[*idx].kind == NodeType::Server)
        .map(|idx| {
            (
                graph[idx].id,
                (
                    graph
                        .neighbors(idx)
                        .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::User)
                        .count(),
                    graph
                        .neighbors(idx)
                        .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::Room)
                        .count(),
                ),
            )
        })
        .collect()
}

// returns map from room id to number of users and servers
pub fn users_servers_per_room_distribution(graph: &Graph) -> HashMap<u64, (usize, usize)> {
    graph
        .node_indices()
        .filter(|idx| graph[*idx].kind == NodeType::Room)
        .map(|idx| {
            (
                graph[idx].id,
                (
                    graph
                        .neighbors(idx)
                        .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::User)
                        .count(),
                    graph
                        .neighbors(idx)
                        .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::Server)
                        .count(),
                ),
            )
        })
        .collect()
}

// returns map from user id to number of rooms (servers per user makes no sense as that's 1:n)
pub fn rooms_per_user_distribution(graph: &Graph) -> HashMap<u64, usize> {
    graph
        .node_indices()
        .filter(|idx| graph[*idx].kind == NodeType::User)
        .map(|idx| {
            (
                graph[idx].id,
                graph
                    .neighbors(idx)
                    .filter(|&neighbor_idx| graph[neighbor_idx].kind == NodeType::Room)
                    .count(),
            )
        })
        .collect()
}
