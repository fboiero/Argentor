//! Git-like conversation tree with branching, forking, and comparison.
//!
//! Each conversation is a tree of message nodes. The "main" branch is the
//! primary conversation path. Users can fork at any point to explore
//! alternatives, then compare or merge results.
//!
//! # Concepts
//!
//! - **Node**: A single message in the tree, linked to a parent and zero or more children.
//! - **Branch**: A named pointer to a head node (like a Git branch).
//! - **Fork**: Creating a new branch from an existing node to explore an alternative path.
//! - **Merge**: Appending a summary message from one branch into the current branch.
//! - **Checkout**: Switching the active branch so new messages go to a different path.
//!
//! # Examples
//!
//! ```
//! use argentor_session::conversation_tree::ConversationTree;
//! use argentor_core::{Message, Role};
//! use uuid::Uuid;
//!
//! let mut tree = ConversationTree::new();
//! let sid = tree.id;
//!
//! // Add messages to the main branch
//! tree.add_message(Message::user("Hello", sid));
//! tree.add_message(Message::assistant("Hi there!", sid));
//!
//! // Fork to try a different prompt
//! tree.fork_here("experiment").unwrap();
//! tree.add_message(Message::user("Try something different", sid));
//!
//! // Switch back to main
//! tree.checkout("main").unwrap();
//! assert_eq!(tree.active_history().len(), 2);
//! ```

use argentor_core::{ArgentorError, ArgentorResult, Message};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use uuid::Uuid;

use crate::Session;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A tree-structured conversation supporting branches, forks, and merges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTree {
    /// Unique identifier (also used as the session_id for generated messages).
    pub id: Uuid,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// All nodes in the tree, keyed by node ID.
    nodes: HashMap<Uuid, ConversationNode>,
    /// Branch name to head node ID mapping.
    branches: HashMap<String, Uuid>,
    /// The currently active branch name.
    active_branch: String,
    /// The ID of the root sentinel node.
    root_id: Uuid,
    /// When this tree was created.
    created_at: DateTime<Utc>,
    /// Last modification timestamp.
    updated_at: DateTime<Utc>,
}

/// A single node in the conversation tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationNode {
    /// Unique identifier for this node.
    pub id: Uuid,
    /// Parent node ID (`None` only for the root sentinel).
    pub parent: Option<Uuid>,
    /// The message stored at this node.
    pub message: Message,
    /// IDs of child nodes.
    pub children: Vec<Uuid>,
    /// Which branch this node was originally created on.
    pub branch: String,
    /// Arbitrary metadata for this node.
    pub metadata: HashMap<String, serde_json::Value>,
    /// When this node was created.
    pub created_at: DateTime<Utc>,
}

/// Summary information about a branch.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    /// Branch name.
    pub name: String,
    /// ID of the head (most recent) node.
    pub head_id: Uuid,
    /// Number of messages in this branch (root to head).
    pub message_count: usize,
    /// Whether this is the currently active branch.
    pub is_active: bool,
    /// The node where this branch diverged from its parent branch (if not root).
    pub fork_point: Option<Uuid>,
    /// When the head node was created (proxy for branch activity).
    pub created_at: DateTime<Utc>,
}

/// Result of comparing two branches.
#[derive(Debug, Clone)]
pub struct BranchComparison {
    /// The common ancestor node of both branches, if any.
    pub common_ancestor: Option<Uuid>,
    /// Node IDs unique to branch A (after the common ancestor).
    pub branch_a_unique: Vec<Uuid>,
    /// Node IDs unique to branch B (after the common ancestor).
    pub branch_b_unique: Vec<Uuid>,
    /// Number of shared messages (from root to common ancestor inclusive).
    pub common_messages: usize,
    /// The node where the branches diverge (same as common ancestor).
    pub divergence_point: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl ConversationTree {
    /// Create a new conversation tree with a "main" branch and a root sentinel node.
    pub fn new() -> Self {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let root_id = Uuid::new_v4();

        let root_node = ConversationNode {
            id: root_id,
            parent: None,
            message: Message::system("conversation root", id),
            children: Vec::new(),
            branch: "main".to_string(),
            metadata: HashMap::new(),
            created_at: now,
        };

        let mut nodes = HashMap::new();
        nodes.insert(root_id, root_node);

        let mut branches = HashMap::new();
        branches.insert("main".to_string(), root_id);

        Self {
            id,
            title: None,
            nodes,
            branches,
            active_branch: "main".to_string(),
            root_id,
            created_at: now,
            updated_at: now,
        }
    }

    // -- Mutation -----------------------------------------------------------

    /// Add a message to the active branch. Returns the new node's ID.
    pub fn add_message(&mut self, message: Message) -> Uuid {
        let head_id = self.branches[&self.active_branch];
        let node_id = Uuid::new_v4();
        let now = Utc::now();

        let node = ConversationNode {
            id: node_id,
            parent: Some(head_id),
            message,
            children: Vec::new(),
            branch: self.active_branch.clone(),
            metadata: HashMap::new(),
            created_at: now,
        };

        self.nodes.insert(node_id, node);

        // Update parent's children list.
        if let Some(parent) = self.nodes.get_mut(&head_id) {
            parent.children.push(node_id);
        }

        // Advance the branch head.
        self.branches.insert(self.active_branch.clone(), node_id);
        self.updated_at = now;

        node_id
    }

    /// Fork: create a new branch starting from a specific node.
    pub fn fork(&mut self, from_node: Uuid, branch_name: impl Into<String>) -> ArgentorResult<()> {
        let name = branch_name.into();

        if self.branches.contains_key(&name) {
            return Err(ArgentorError::Session(format!(
                "branch '{name}' already exists"
            )));
        }
        if !self.nodes.contains_key(&from_node) {
            return Err(ArgentorError::Session(format!(
                "node {from_node} not found"
            )));
        }

        self.branches.insert(name.clone(), from_node);
        self.active_branch = name;
        self.updated_at = Utc::now();
        Ok(())
    }

    /// Fork from the current branch's head node.
    pub fn fork_here(&mut self, branch_name: impl Into<String>) -> ArgentorResult<()> {
        let head_id = self.branches[&self.active_branch];
        self.fork(head_id, branch_name)
    }

    /// Switch the active branch.
    pub fn checkout(&mut self, branch: &str) -> ArgentorResult<()> {
        if !self.branches.contains_key(branch) {
            return Err(ArgentorError::Session(format!(
                "branch '{branch}' does not exist"
            )));
        }
        self.active_branch = branch.to_string();
        self.updated_at = Utc::now();
        Ok(())
    }

    /// Merge another branch into the current branch by appending a summary message.
    /// Returns the ID of the newly created merge node.
    pub fn merge_branch(&mut self, source_branch: &str, summary: &str) -> ArgentorResult<Uuid> {
        if !self.branches.contains_key(source_branch) {
            return Err(ArgentorError::Session(format!(
                "branch '{source_branch}' does not exist"
            )));
        }
        if source_branch == self.active_branch {
            return Err(ArgentorError::Session(
                "cannot merge a branch into itself".to_string(),
            ));
        }

        let merge_content = format!("[merge from '{source_branch}'] {summary}");
        let merge_msg = Message::system(merge_content, self.id);
        let node_id = self.add_message(merge_msg);

        // Store merge metadata on the new node.
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.metadata.insert(
                "merge_source".to_string(),
                serde_json::Value::String(source_branch.to_string()),
            );
            let source_head = self.branches[source_branch];
            node.metadata.insert(
                "merge_source_head".to_string(),
                serde_json::Value::String(source_head.to_string()),
            );
        }

        Ok(node_id)
    }

    /// Delete a branch. Cannot delete "main" or the currently active branch.
    pub fn delete_branch(&mut self, branch: &str) -> ArgentorResult<()> {
        if branch == "main" {
            return Err(ArgentorError::Session(
                "cannot delete the 'main' branch".to_string(),
            ));
        }
        if branch == self.active_branch {
            return Err(ArgentorError::Session(
                "cannot delete the active branch; checkout another branch first".to_string(),
            ));
        }
        if self.branches.remove(branch).is_none() {
            return Err(ArgentorError::Session(format!(
                "branch '{branch}' does not exist"
            )));
        }
        self.updated_at = Utc::now();
        Ok(())
    }

    // -- Queries ------------------------------------------------------------

    /// Get the linear message history of the active branch (root sentinel excluded).
    pub fn active_history(&self) -> Vec<&Message> {
        self.history_for_head(self.branches[&self.active_branch])
    }

    /// Get the linear message history of a named branch (root sentinel excluded).
    pub fn branch_history(&self, branch: &str) -> Option<Vec<&Message>> {
        let head_id = self.branches.get(branch)?;
        Some(self.history_for_head(*head_id))
    }

    /// List all branches with summary information.
    pub fn branches(&self) -> Vec<BranchInfo> {
        self.branches
            .iter()
            .map(|(name, &head_id)| {
                let history = self.path_to_root(head_id);
                // The fork point is the first node in the path that belongs
                // to a different branch (or root if the branch started at root).
                let fork_point = history
                    .iter()
                    .rev()
                    .find(|&&nid| {
                        let node = &self.nodes[&nid];
                        node.branch != *name && nid != self.root_id
                    })
                    .copied();

                let head_node = &self.nodes[&head_id];
                // message_count excludes the root sentinel
                let message_count = history.iter().filter(|&&nid| nid != self.root_id).count();

                BranchInfo {
                    name: name.clone(),
                    head_id,
                    message_count,
                    is_active: *name == self.active_branch,
                    fork_point,
                    created_at: head_node.created_at,
                }
            })
            .collect()
    }

    /// Compare two branches, producing unique and shared node lists.
    pub fn compare_branches(&self, branch_a: &str, branch_b: &str) -> BranchComparison {
        let empty = BranchComparison {
            common_ancestor: None,
            branch_a_unique: Vec::new(),
            branch_b_unique: Vec::new(),
            common_messages: 0,
            divergence_point: None,
        };

        let head_a = match self.branches.get(branch_a) {
            Some(id) => *id,
            None => return empty,
        };
        let head_b = match self.branches.get(branch_b) {
            Some(id) => *id,
            None => return empty,
        };

        let path_a = self.path_to_root(head_a); // head → root order
        let path_b = self.path_to_root(head_b);

        let set_b: HashSet<Uuid> = path_b.iter().copied().collect();

        // Find common ancestor: walk from head_a towards root, first node in set_b.
        let common_ancestor = path_a.iter().find(|id| set_b.contains(id)).copied();

        let common_set: HashSet<Uuid> = if let Some(ca) = common_ancestor {
            // All nodes from common ancestor to root (inclusive).
            let ca_path = self.path_to_root(ca);
            ca_path.into_iter().collect()
        } else {
            HashSet::new()
        };

        let branch_a_unique: Vec<Uuid> = path_a
            .iter()
            .filter(|id| !common_set.contains(id))
            .copied()
            .collect();
        let branch_b_unique: Vec<Uuid> = path_b
            .iter()
            .filter(|id| !common_set.contains(id))
            .copied()
            .collect();

        // Common messages excludes the root sentinel.
        let common_messages = common_set.iter().filter(|&&id| id != self.root_id).count();

        BranchComparison {
            common_ancestor,
            branch_a_unique,
            branch_b_unique,
            common_messages,
            divergence_point: common_ancestor,
        }
    }

    /// Get the common ancestor of two branches, if both exist.
    pub fn common_ancestor(&self, branch_a: &str, branch_b: &str) -> Option<Uuid> {
        let head_a = *self.branches.get(branch_a)?;
        let head_b = *self.branches.get(branch_b)?;

        let path_b: HashSet<Uuid> = self.path_to_root(head_b).into_iter().collect();
        self.path_to_root(head_a)
            .into_iter()
            .find(|id| path_b.contains(id))
    }

    /// Total number of messages across all branches (excluding the root sentinel).
    pub fn total_messages(&self) -> usize {
        self.nodes.len() - 1 // minus the root sentinel
    }

    /// Look up a node by ID.
    pub fn get_node(&self, id: &Uuid) -> Option<&ConversationNode> {
        self.nodes.get(id)
    }

    /// Get the name of the active branch.
    pub fn active_branch_name(&self) -> &str {
        &self.active_branch
    }

    /// Get the root node ID.
    pub fn root_id(&self) -> Uuid {
        self.root_id
    }

    // -- Conversion ---------------------------------------------------------

    /// Convert the active branch into a flat [`Session`] for compatibility
    /// with [`AgentRunner`] and other linear-session consumers.
    pub fn to_session(&self) -> Session {
        let messages = self.active_history().into_iter().cloned().collect();
        let now = Utc::now();
        Session {
            id: self.id,
            messages,
            active_skills: Vec::new(),
            created_at: self.created_at,
            updated_at: now,
            metadata: HashMap::new(),
        }
    }

    /// Import a [`Session`] as the "main" branch of a new tree.
    pub fn from_session(session: &Session) -> Self {
        let mut tree = Self {
            id: session.id,
            title: None,
            nodes: HashMap::new(),
            branches: HashMap::new(),
            active_branch: "main".to_string(),
            root_id: Uuid::new_v4(),
            created_at: session.created_at,
            updated_at: session.updated_at,
        };

        // Insert root sentinel.
        let root = ConversationNode {
            id: tree.root_id,
            parent: None,
            message: Message::system("conversation root", session.id),
            children: Vec::new(),
            branch: "main".to_string(),
            metadata: HashMap::new(),
            created_at: session.created_at,
        };
        tree.nodes.insert(tree.root_id, root);
        tree.branches.insert("main".to_string(), tree.root_id);

        // Re-insert each message from the session.
        for msg in &session.messages {
            tree.add_message(msg.clone());
        }

        tree
    }

    // -- Persistence --------------------------------------------------------

    /// Save the tree to a JSON file.
    pub fn save(&self, path: &Path) -> ArgentorResult<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a tree from a JSON file.
    pub fn load(path: &Path) -> ArgentorResult<Self> {
        let data = std::fs::read_to_string(path)?;
        let tree: Self = serde_json::from_str(&data)?;
        Ok(tree)
    }

    // -- Internal helpers ---------------------------------------------------

    /// Walk from `node_id` to the root, returning IDs in head-to-root order.
    fn path_to_root(&self, node_id: Uuid) -> Vec<Uuid> {
        let mut path = Vec::new();
        let mut current = Some(node_id);
        while let Some(id) = current {
            path.push(id);
            current = self.nodes.get(&id).and_then(|n| n.parent);
        }
        path
    }

    /// Build the linear message history for a head node (root sentinel excluded),
    /// returned in chronological order (root → head).
    fn history_for_head(&self, head_id: Uuid) -> Vec<&Message> {
        let path = self.path_to_root(head_id);
        path.into_iter()
            .rev()
            .filter(|id| *id != self.root_id)
            .filter_map(|id| self.nodes.get(&id).map(|n| &n.message))
            .collect()
    }
}

impl Default for ConversationTree {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use argentor_core::Role;

    /// Helper to create a user message for a tree.
    fn user_msg(tree: &ConversationTree, content: &str) -> Message {
        Message::user(content, tree.id)
    }

    /// Helper to create an assistant message for a tree.
    fn asst_msg(tree: &ConversationTree, content: &str) -> Message {
        Message::assistant(content, tree.id)
    }

    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    #[test]
    fn new_tree_has_main_branch() {
        let tree = ConversationTree::new();
        assert_eq!(tree.active_branch_name(), "main");
        assert!(tree.branches.contains_key("main"));
        assert_eq!(tree.total_messages(), 0);
    }

    #[test]
    fn new_tree_has_root_node() {
        let tree = ConversationTree::new();
        let root = tree.get_node(&tree.root_id()).unwrap();
        assert!(root.parent.is_none());
        assert_eq!(root.branch, "main");
    }

    #[test]
    fn new_tree_active_history_is_empty() {
        let tree = ConversationTree::new();
        assert!(tree.active_history().is_empty());
    }

    // -----------------------------------------------------------------------
    // Adding messages
    // -----------------------------------------------------------------------

    #[test]
    fn add_single_message() {
        let mut tree = ConversationTree::new();
        let msg = user_msg(&tree, "hello");
        let nid = tree.add_message(msg);
        assert_eq!(tree.total_messages(), 1);
        let node = tree.get_node(&nid).unwrap();
        assert_eq!(node.message.content, "hello");
        assert_eq!(node.branch, "main");
    }

    #[test]
    fn add_multiple_messages_linear_history() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "one"));
        tree.add_message(asst_msg(&tree, "two"));
        tree.add_message(user_msg(&tree, "three"));

        let history = tree.active_history();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].content, "one");
        assert_eq!(history[1].content, "two");
        assert_eq!(history[2].content, "three");
    }

    #[test]
    fn parent_child_links_correct() {
        let mut tree = ConversationTree::new();
        let n1 = tree.add_message(user_msg(&tree, "first"));
        let n2 = tree.add_message(user_msg(&tree, "second"));

        let node1 = tree.get_node(&n1).unwrap();
        let node2 = tree.get_node(&n2).unwrap();

        assert_eq!(node1.parent, Some(tree.root_id()));
        assert_eq!(node2.parent, Some(n1));
        assert!(node1.children.contains(&n2));
    }

    // -----------------------------------------------------------------------
    // Forking
    // -----------------------------------------------------------------------

    #[test]
    fn fork_creates_new_branch() {
        let mut tree = ConversationTree::new();
        let n1 = tree.add_message(user_msg(&tree, "base"));
        tree.fork(n1, "experiment").unwrap();
        assert_eq!(tree.active_branch_name(), "experiment");
        assert!(tree.branches.contains_key("experiment"));
    }

    #[test]
    fn fork_here_creates_branch_at_head() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "one"));
        let n2 = tree.add_message(user_msg(&tree, "two"));
        tree.fork_here("alt").unwrap();

        // The new branch head should be the same as the old head.
        assert_eq!(tree.branches["alt"], n2);
        assert_eq!(tree.active_branch_name(), "alt");
    }

    #[test]
    fn fork_duplicate_name_errors() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "x"));
        let result = tree.fork_here("main");
        assert!(result.is_err());
    }

    #[test]
    fn fork_nonexistent_node_errors() {
        let mut tree = ConversationTree::new();
        let bad_id = Uuid::new_v4();
        let result = tree.fork(bad_id, "nope");
        assert!(result.is_err());
    }

    #[test]
    fn multiple_forks_from_same_node() {
        let mut tree = ConversationTree::new();
        let n1 = tree.add_message(user_msg(&tree, "base"));
        tree.fork(n1, "alt-a").unwrap();
        tree.checkout("main").unwrap();
        tree.fork(n1, "alt-b").unwrap();

        assert_eq!(tree.branches().len(), 3); // main, alt-a, alt-b
    }

    // -----------------------------------------------------------------------
    // Checkout
    // -----------------------------------------------------------------------

    #[test]
    fn checkout_switches_branch() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "main msg"));
        tree.fork_here("other").unwrap();
        tree.checkout("main").unwrap();
        assert_eq!(tree.active_branch_name(), "main");
    }

    #[test]
    fn checkout_nonexistent_branch_errors() {
        let tree = ConversationTree::new();
        assert!(tree.clone().checkout("ghost").is_err());
    }

    #[test]
    fn messages_go_to_active_branch() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "main-1"));
        tree.fork_here("alt").unwrap();
        tree.add_message(user_msg(&tree, "alt-1"));
        tree.add_message(user_msg(&tree, "alt-2"));

        // Alt branch should have main-1 + alt-1 + alt-2
        let alt_hist = tree.active_history();
        assert_eq!(alt_hist.len(), 3);
        assert_eq!(alt_hist[0].content, "main-1");
        assert_eq!(alt_hist[1].content, "alt-1");
        assert_eq!(alt_hist[2].content, "alt-2");

        // Main branch should only have main-1
        tree.checkout("main").unwrap();
        let main_hist = tree.active_history();
        assert_eq!(main_hist.len(), 1);
        assert_eq!(main_hist[0].content, "main-1");
    }

    // -----------------------------------------------------------------------
    // Branch history
    // -----------------------------------------------------------------------

    #[test]
    fn branch_history_returns_correct_messages() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "shared"));
        tree.fork_here("dev").unwrap();
        tree.add_message(user_msg(&tree, "dev-only"));

        let dev_hist = tree.branch_history("dev").unwrap();
        assert_eq!(dev_hist.len(), 2);
        assert_eq!(dev_hist[1].content, "dev-only");

        let main_hist = tree.branch_history("main").unwrap();
        assert_eq!(main_hist.len(), 1);
    }

    #[test]
    fn branch_history_nonexistent_returns_none() {
        let tree = ConversationTree::new();
        assert!(tree.branch_history("nope").is_none());
    }

    // -----------------------------------------------------------------------
    // Compare branches
    // -----------------------------------------------------------------------

    #[test]
    fn compare_branches_finds_common_ancestor() {
        let mut tree = ConversationTree::new();
        let shared = tree.add_message(user_msg(&tree, "shared"));
        tree.fork(shared, "alt").unwrap();
        tree.add_message(user_msg(&tree, "alt-msg"));

        tree.checkout("main").unwrap();
        tree.add_message(user_msg(&tree, "main-msg"));

        let cmp = tree.compare_branches("main", "alt");
        assert!(cmp.common_ancestor.is_some());
        assert_eq!(cmp.common_ancestor.unwrap(), shared);
        assert_eq!(cmp.branch_a_unique.len(), 1);
        assert_eq!(cmp.branch_b_unique.len(), 1);
        assert_eq!(cmp.common_messages, 1); // "shared" node
    }

    #[test]
    fn compare_branches_nonexistent_returns_empty() {
        let tree = ConversationTree::new();
        let cmp = tree.compare_branches("main", "ghost");
        assert!(cmp.common_ancestor.is_none());
        assert!(cmp.branch_a_unique.is_empty());
    }

    #[test]
    fn compare_same_branch_all_common() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "a"));
        tree.add_message(user_msg(&tree, "b"));

        let cmp = tree.compare_branches("main", "main");
        assert!(cmp.branch_a_unique.is_empty());
        assert!(cmp.branch_b_unique.is_empty());
        assert_eq!(cmp.common_messages, 2);
    }

    // -----------------------------------------------------------------------
    // Common ancestor
    // -----------------------------------------------------------------------

    #[test]
    fn common_ancestor_basic() {
        let mut tree = ConversationTree::new();
        let base = tree.add_message(user_msg(&tree, "base"));
        tree.fork(base, "side").unwrap();
        tree.add_message(user_msg(&tree, "side-1"));

        let ca = tree.common_ancestor("main", "side");
        assert_eq!(ca, Some(base));
    }

    #[test]
    fn common_ancestor_nonexistent_branch() {
        let tree = ConversationTree::new();
        assert!(tree.common_ancestor("main", "nope").is_none());
    }

    // -----------------------------------------------------------------------
    // Merge
    // -----------------------------------------------------------------------

    #[test]
    fn merge_branch_adds_summary_message() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "shared"));
        tree.fork_here("experiment").unwrap();
        tree.add_message(asst_msg(&tree, "experiment result"));

        tree.checkout("main").unwrap();
        let merge_id = tree
            .merge_branch("experiment", "Best result from experiment")
            .unwrap();

        let node = tree.get_node(&merge_id).unwrap();
        assert!(node.message.content.contains("merge from 'experiment'"));
        assert!(node.message.content.contains("Best result from experiment"));
        assert_eq!(node.message.role, Role::System);
        assert!(node.metadata.contains_key("merge_source"));
    }

    #[test]
    fn merge_nonexistent_branch_errors() {
        let mut tree = ConversationTree::new();
        assert!(tree.merge_branch("ghost", "x").is_err());
    }

    #[test]
    fn merge_self_errors() {
        let mut tree = ConversationTree::new();
        assert!(tree.merge_branch("main", "x").is_err());
    }

    // -----------------------------------------------------------------------
    // Delete branch
    // -----------------------------------------------------------------------

    #[test]
    fn delete_branch_removes_it() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "x"));
        tree.fork_here("temp").unwrap();
        tree.checkout("main").unwrap();
        tree.delete_branch("temp").unwrap();
        assert!(!tree.branches.contains_key("temp"));
    }

    #[test]
    fn delete_main_branch_errors() {
        let mut tree = ConversationTree::new();
        assert!(tree.delete_branch("main").is_err());
    }

    #[test]
    fn delete_active_branch_errors() {
        let mut tree = ConversationTree::new();
        tree.fork_here("active").unwrap();
        // "active" is now the active branch
        assert!(tree.delete_branch("active").is_err());
    }

    #[test]
    fn delete_nonexistent_branch_errors() {
        let mut tree = ConversationTree::new();
        assert!(tree.delete_branch("nope").is_err());
    }

    // -----------------------------------------------------------------------
    // Session roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn to_session_contains_active_messages() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "hello"));
        tree.add_message(asst_msg(&tree, "world"));

        let session = tree.to_session();
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].content, "hello");
        assert_eq!(session.messages[1].content, "world");
        assert_eq!(session.id, tree.id);
    }

    #[test]
    fn from_session_preserves_messages() {
        let mut session = Session::new();
        session.add_message(Message::user("a", session.id));
        session.add_message(Message::assistant("b", session.id));

        let tree = ConversationTree::from_session(&session);
        assert_eq!(tree.id, session.id);
        let history = tree.active_history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "a");
        assert_eq!(history[1].content, "b");
    }

    #[test]
    fn session_roundtrip_preserves_content() {
        let mut original = Session::new();
        original.add_message(Message::user("q1", original.id));
        original.add_message(Message::assistant("a1", original.id));
        original.add_message(Message::user("q2", original.id));

        let tree = ConversationTree::from_session(&original);
        let recovered = tree.to_session();

        assert_eq!(recovered.messages.len(), original.messages.len());
        for (orig, recov) in original.messages.iter().zip(recovered.messages.iter()) {
            assert_eq!(orig.content, recov.content);
            assert_eq!(orig.role, recov.role);
        }
    }

    // -----------------------------------------------------------------------
    // Branch info listing
    // -----------------------------------------------------------------------

    #[test]
    fn branches_lists_all_branches() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "a"));
        tree.fork_here("b1").unwrap();
        tree.checkout("main").unwrap();
        tree.fork_here("b2").unwrap();

        let infos = tree.branches();
        assert_eq!(infos.len(), 3);

        let names: Vec<String> = infos.iter().map(|b| b.name.clone()).collect();
        assert!(names.contains(&"main".to_string()));
        assert!(names.contains(&"b1".to_string()));
        assert!(names.contains(&"b2".to_string()));
    }

    #[test]
    fn branch_info_active_flag() {
        let mut tree = ConversationTree::new();
        tree.fork_here("side").unwrap();

        let infos = tree.branches();
        let side = infos.iter().find(|b| b.name == "side").unwrap();
        let main = infos.iter().find(|b| b.name == "main").unwrap();

        assert!(side.is_active);
        assert!(!main.is_active);
    }

    #[test]
    fn branch_info_message_count() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "shared"));
        tree.fork_here("alt").unwrap();
        tree.add_message(user_msg(&tree, "alt-1"));
        tree.add_message(user_msg(&tree, "alt-2"));

        let infos = tree.branches();
        let alt = infos.iter().find(|b| b.name == "alt").unwrap();
        let main = infos.iter().find(|b| b.name == "main").unwrap();

        assert_eq!(alt.message_count, 3); // shared + alt-1 + alt-2
        assert_eq!(main.message_count, 1); // shared
    }

    // -----------------------------------------------------------------------
    // Persistence
    // -----------------------------------------------------------------------

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tree.json");

        let mut tree = ConversationTree::new();
        tree.title = Some("test tree".to_string());
        tree.add_message(user_msg(&tree, "msg1"));
        tree.add_message(asst_msg(&tree, "msg2"));
        tree.fork_here("branch2").unwrap();
        tree.add_message(user_msg(&tree, "msg3"));

        tree.save(&path).unwrap();
        let loaded = ConversationTree::load(&path).unwrap();

        assert_eq!(loaded.id, tree.id);
        assert_eq!(loaded.title, Some("test tree".to_string()));
        assert_eq!(loaded.total_messages(), tree.total_messages());
        assert_eq!(loaded.active_branch_name(), "branch2");

        let hist = loaded.active_history();
        assert_eq!(hist.len(), 3);
        assert_eq!(hist[2].content, "msg3");
    }

    #[test]
    fn load_nonexistent_file_errors() {
        let result = ConversationTree::load(Path::new("/tmp/nonexistent_argentor_tree.json"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Total message count
    // -----------------------------------------------------------------------

    #[test]
    fn total_messages_across_branches() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "shared"));
        tree.fork_here("alt").unwrap();
        tree.add_message(user_msg(&tree, "alt-1"));
        tree.checkout("main").unwrap();
        tree.add_message(user_msg(&tree, "main-2"));

        // shared + alt-1 + main-2 = 3 unique nodes
        assert_eq!(tree.total_messages(), 3);
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn empty_tree_to_session_is_empty() {
        let tree = ConversationTree::new();
        let session = tree.to_session();
        assert!(session.messages.is_empty());
    }

    #[test]
    fn from_empty_session() {
        let session = Session::new();
        let tree = ConversationTree::from_session(&session);
        assert_eq!(tree.total_messages(), 0);
        assert!(tree.active_history().is_empty());
    }

    #[test]
    fn default_impl_works() {
        let tree = ConversationTree::default();
        assert_eq!(tree.active_branch_name(), "main");
        assert_eq!(tree.total_messages(), 0);
    }

    #[test]
    fn deep_chain_history_order() {
        let mut tree = ConversationTree::new();
        for i in 0..20 {
            tree.add_message(user_msg(&tree, &format!("msg-{}", i)));
        }
        let history = tree.active_history();
        assert_eq!(history.len(), 20);
        for (i, msg) in history.iter().enumerate() {
            assert_eq!(msg.content, format!("msg-{}", i));
        }
    }

    #[test]
    fn fork_preserves_shared_history() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "s1"));
        tree.add_message(user_msg(&tree, "s2"));
        tree.fork_here("alt").unwrap();
        tree.add_message(user_msg(&tree, "a1"));

        // Alt branch sees shared + own
        let alt = tree.active_history();
        assert_eq!(alt.len(), 3);
        assert_eq!(alt[0].content, "s1");
        assert_eq!(alt[1].content, "s2");
        assert_eq!(alt[2].content, "a1");

        // Main unchanged
        let main = tree.branch_history("main").unwrap();
        assert_eq!(main.len(), 2);
    }

    #[test]
    fn merge_then_continue() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "base"));
        tree.fork_here("exp").unwrap();
        tree.add_message(asst_msg(&tree, "experiment result"));

        tree.checkout("main").unwrap();
        tree.merge_branch("exp", "good result").unwrap();
        tree.add_message(user_msg(&tree, "continuing after merge"));

        let hist = tree.active_history();
        assert_eq!(hist.len(), 3); // base + merge msg + continuing
        assert!(hist[1].content.contains("merge from 'exp'"));
        assert_eq!(hist[2].content, "continuing after merge");
    }

    #[test]
    fn compare_diverged_branches_correct_counts() {
        let mut tree = ConversationTree::new();
        tree.add_message(user_msg(&tree, "s1"));
        tree.add_message(user_msg(&tree, "s2"));

        tree.fork_here("alt").unwrap();
        tree.add_message(user_msg(&tree, "a1"));
        tree.add_message(user_msg(&tree, "a2"));
        tree.add_message(user_msg(&tree, "a3"));

        tree.checkout("main").unwrap();
        tree.add_message(user_msg(&tree, "m1"));

        let cmp = tree.compare_branches("main", "alt");
        assert_eq!(cmp.common_messages, 2); // s1, s2
        assert_eq!(cmp.branch_a_unique.len(), 1); // m1
        assert_eq!(cmp.branch_b_unique.len(), 3); // a1, a2, a3
    }
}
