//! In-memory knowledge graph for entity-relationship storage.
//!
//! Tracks entities (people, concepts, tools, files), relationships between them,
//! and facts with temporal metadata. Supports querying by entity, relationship type,
//! and neighborhood traversal.
//!
//! # Main types
//!
//! - [`Entity`] — A node in the knowledge graph.
//! - [`Relationship`] — A directed edge between two entities.
//! - [`KnowledgeGraph`] — The graph container with CRUD, traversal, and persistence.
//! - [`GraphSummary`] — Aggregate statistics about the graph.

use argentor_core::{ArgentorError, ArgentorResult};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Entity types
// ---------------------------------------------------------------------------

/// A node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Unique identifier for this entity.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The kind of entity (Person, Concept, Tool, etc.).
    pub entity_type: EntityType,
    /// Arbitrary key-value properties associated with this entity.
    pub properties: HashMap<String, serde_json::Value>,
    /// When this entity was first created.
    pub created_at: DateTime<Utc>,
    /// When this entity was last modified.
    pub updated_at: DateTime<Utc>,
    /// Confidence score (0.0 -- 1.0) representing extraction reliability.
    pub confidence: f64,
    /// Origin of this entity: "user", "agent", "tool_result", "extracted".
    pub source: String,
}

/// Classification of entity types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EntityType {
    /// A human individual.
    Person,
    /// A company, team, or institution.
    Organization,
    /// An abstract concept or topic.
    Concept,
    /// A tool, skill, or command.
    Tool,
    /// A file or path on disk.
    File,
    /// A geographic or network location.
    Location,
    /// A discrete event with temporal bounds.
    Event,
    /// A standalone factual assertion.
    Fact,
    /// Application-defined entity type.
    Custom(String),
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Person => write!(f, "Person"),
            Self::Organization => write!(f, "Organization"),
            Self::Concept => write!(f, "Concept"),
            Self::Tool => write!(f, "Tool"),
            Self::File => write!(f, "File"),
            Self::Location => write!(f, "Location"),
            Self::Event => write!(f, "Event"),
            Self::Fact => write!(f, "Fact"),
            Self::Custom(s) => write!(f, "Custom({s})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Relationship types
// ---------------------------------------------------------------------------

/// A directed edge between two entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    /// Unique identifier for this relationship.
    pub id: String,
    /// Source entity ID.
    pub from_entity: String,
    /// Target entity ID.
    pub to_entity: String,
    /// Kind of relationship.
    pub relation_type: RelationType,
    /// Arbitrary properties on the edge.
    pub properties: HashMap<String, serde_json::Value>,
    /// Importance / confidence weight (0.0 -- 1.0).
    pub weight: f64,
    /// When this relationship was created.
    pub created_at: DateTime<Utc>,
    /// Origin of this relationship.
    pub source: String,
}

/// Classification of relationship types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RelationType {
    /// Subsumption: "Dog IsA Animal".
    IsA,
    /// Property attribution: "User HasProperty email".
    HasProperty,
    /// Generic association.
    RelatedTo,
    /// Dependency: "TaskA DependsOn TaskB".
    DependsOn,
    /// Authorship: "Report CreatedBy Agent".
    CreatedBy,
    /// Containment: "Project Contains File".
    Contains,
    /// Collaboration: "Alice WorksWith Bob".
    WorksWith,
    /// Reference: "Message Mentions Entity".
    Mentions,
    /// Tool usage: "Agent UsedTool calculator".
    UsedTool,
    /// Output production: "Step ProducedOutput artifact".
    ProducedOutput,
    /// Application-defined relationship type.
    Custom(String),
}

impl std::fmt::Display for RelationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IsA => write!(f, "IsA"),
            Self::HasProperty => write!(f, "HasProperty"),
            Self::RelatedTo => write!(f, "RelatedTo"),
            Self::DependsOn => write!(f, "DependsOn"),
            Self::CreatedBy => write!(f, "CreatedBy"),
            Self::Contains => write!(f, "Contains"),
            Self::WorksWith => write!(f, "WorksWith"),
            Self::Mentions => write!(f, "Mentions"),
            Self::UsedTool => write!(f, "UsedTool"),
            Self::ProducedOutput => write!(f, "ProducedOutput"),
            Self::Custom(s) => write!(f, "Custom({s})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Graph summary
// ---------------------------------------------------------------------------

/// Aggregate statistics about the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSummary {
    /// Total number of entities.
    pub entity_count: usize,
    /// Total number of relationships.
    pub relationship_count: usize,
    /// Count of entities by type.
    pub entity_types: HashMap<String, usize>,
    /// Count of relationships by type.
    pub relationship_types: HashMap<String, usize>,
    /// Top entities by total connection count (name, count), descending.
    pub most_connected: Vec<(String, usize)>,
}

// ---------------------------------------------------------------------------
// Serializable graph for persistence
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct SerializableGraph {
    entities: Vec<Entity>,
    relationships: Vec<Relationship>,
}

// ---------------------------------------------------------------------------
// KnowledgeGraph
// ---------------------------------------------------------------------------

/// In-memory knowledge graph with indexed lookups and graph traversal.
pub struct KnowledgeGraph {
    entities: HashMap<String, Entity>,
    relationships: Vec<Relationship>,
    // Indexes
    entity_by_name: HashMap<String, Vec<String>>,
    entity_by_type: HashMap<EntityType, Vec<String>>,
    relations_from: HashMap<String, Vec<usize>>,
    relations_to: HashMap<String, Vec<usize>>,
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl KnowledgeGraph {
    /// Create an empty knowledge graph.
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            relationships: Vec::new(),
            entity_by_name: HashMap::new(),
            entity_by_type: HashMap::new(),
            relations_from: HashMap::new(),
            relations_to: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Entity CRUD
    // -----------------------------------------------------------------------

    /// Add an entity to the graph. Returns the entity ID.
    ///
    /// If the entity's `id` field is empty, a new UUID is generated.
    pub fn add_entity(&mut self, mut entity: Entity) -> String {
        if entity.id.is_empty() {
            entity.id = Uuid::new_v4().to_string();
        }
        let id = entity.id.clone();
        let name_lower = entity.name.to_lowercase();

        // Update name index
        self.entity_by_name
            .entry(name_lower)
            .or_default()
            .push(id.clone());

        // Update type index
        self.entity_by_type
            .entry(entity.entity_type.clone())
            .or_default()
            .push(id.clone());

        self.entities.insert(id.clone(), entity);
        id
    }

    /// Retrieve an entity by ID.
    pub fn get_entity(&self, id: &str) -> Option<&Entity> {
        self.entities.get(id)
    }

    /// Find all entities whose name matches (case-insensitive substring).
    pub fn find_entities(&self, name: &str) -> Vec<&Entity> {
        let query = name.to_lowercase();
        self.entity_by_name
            .iter()
            .filter(|(k, _)| k.contains(&query))
            .flat_map(|(_, ids)| ids.iter().filter_map(|id| self.entities.get(id)))
            .collect()
    }

    /// Find all entities of a given type.
    pub fn find_by_type(&self, entity_type: &EntityType) -> Vec<&Entity> {
        self.entity_by_type
            .get(entity_type)
            .map(|ids| ids.iter().filter_map(|id| self.entities.get(id)).collect())
            .unwrap_or_default()
    }

    /// Update an entity's properties. Returns `true` if the entity was found and updated.
    pub fn update_entity(
        &mut self,
        id: &str,
        properties: HashMap<String, serde_json::Value>,
    ) -> bool {
        if let Some(entity) = self.entities.get_mut(id) {
            for (k, v) in properties {
                entity.properties.insert(k, v);
            }
            entity.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    /// Remove an entity and all its incident relationships.
    /// Returns `true` if the entity existed.
    pub fn remove_entity(&mut self, id: &str) -> bool {
        let entity = match self.entities.remove(id) {
            Some(e) => e,
            None => return false,
        };

        // Remove from name index
        let name_lower = entity.name.to_lowercase();
        if let Some(ids) = self.entity_by_name.get_mut(&name_lower) {
            ids.retain(|i| i != id);
            if ids.is_empty() {
                self.entity_by_name.remove(&name_lower);
            }
        }

        // Remove from type index
        if let Some(ids) = self.entity_by_type.get_mut(&entity.entity_type) {
            ids.retain(|i| i != id);
            if ids.is_empty() {
                self.entity_by_type.remove(&entity.entity_type);
            }
        }

        // Remove incident relationships (rebuild to avoid index invalidation)
        self.relationships
            .retain(|r| r.from_entity != id && r.to_entity != id);
        self.rebuild_relation_indexes();

        true
    }

    // -----------------------------------------------------------------------
    // Relationship CRUD
    // -----------------------------------------------------------------------

    /// Add a relationship to the graph. Returns the relationship ID.
    ///
    /// If the relationship's `id` field is empty, a new UUID is generated.
    pub fn add_relationship(&mut self, mut rel: Relationship) -> String {
        if rel.id.is_empty() {
            rel.id = Uuid::new_v4().to_string();
        }
        let id = rel.id.clone();
        let idx = self.relationships.len();

        self.relations_from
            .entry(rel.from_entity.clone())
            .or_default()
            .push(idx);
        self.relations_to
            .entry(rel.to_entity.clone())
            .or_default()
            .push(idx);

        self.relationships.push(rel);
        id
    }

    /// Get all relationships originating from a given entity.
    pub fn get_relationships_from(&self, entity_id: &str) -> Vec<&Relationship> {
        self.relations_from
            .get(entity_id)
            .map(|idxs| {
                idxs.iter()
                    .filter_map(|&i| self.relationships.get(i))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all relationships pointing to a given entity.
    pub fn get_relationships_to(&self, entity_id: &str) -> Vec<&Relationship> {
        self.relations_to
            .get(entity_id)
            .map(|idxs| {
                idxs.iter()
                    .filter_map(|&i| self.relationships.get(i))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find relationships matching optional filters.
    pub fn find_relationships(
        &self,
        from: Option<&str>,
        to: Option<&str>,
        rel_type: Option<&RelationType>,
    ) -> Vec<&Relationship> {
        self.relationships
            .iter()
            .filter(|r| {
                from.map_or(true, |f| r.from_entity == f)
                    && to.map_or(true, |t| r.to_entity == t)
                    && rel_type.map_or(true, |rt| &r.relation_type == rt)
            })
            .collect()
    }

    /// Remove a relationship by ID. Returns `true` if it was found and removed.
    pub fn remove_relationship(&mut self, id: &str) -> bool {
        let before = self.relationships.len();
        self.relationships.retain(|r| r.id != id);
        if self.relationships.len() < before {
            self.rebuild_relation_indexes();
            true
        } else {
            false
        }
    }

    // -----------------------------------------------------------------------
    // Graph queries
    // -----------------------------------------------------------------------

    /// BFS traversal to collect all neighbor entities within a given depth.
    ///
    /// Treats the graph as undirected for traversal purposes.
    pub fn neighbors(&self, entity_id: &str, depth: usize) -> Vec<&Entity> {
        if !self.entities.contains_key(entity_id) {
            return Vec::new();
        }

        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(entity_id);

        let mut queue: VecDeque<(&str, usize)> = VecDeque::new();
        queue.push_back((entity_id, 0));

        let mut result = Vec::new();

        while let Some((current, d)) = queue.pop_front() {
            if d >= depth {
                continue;
            }

            // Outgoing edges
            if let Some(idxs) = self.relations_from.get(current) {
                for &idx in idxs {
                    if let Some(rel) = self.relationships.get(idx) {
                        let neighbor = rel.to_entity.as_str();
                        if visited.insert(neighbor) {
                            if let Some(entity) = self.entities.get(neighbor) {
                                result.push(entity);
                                queue.push_back((neighbor, d + 1));
                            }
                        }
                    }
                }
            }

            // Incoming edges (undirected traversal)
            if let Some(idxs) = self.relations_to.get(current) {
                for &idx in idxs {
                    if let Some(rel) = self.relationships.get(idx) {
                        let neighbor = rel.from_entity.as_str();
                        if visited.insert(neighbor) {
                            if let Some(entity) = self.entities.get(neighbor) {
                                result.push(entity);
                                queue.push_back((neighbor, d + 1));
                            }
                        }
                    }
                }
            }
        }

        result
    }

    /// BFS shortest path between two entities. Returns the list of entity IDs along the path
    /// (including start and end), or `None` if no path exists.
    ///
    /// Treats the graph as undirected.
    pub fn shortest_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        if from == to {
            return Some(vec![from.to_string()]);
        }
        if !self.entities.contains_key(from) || !self.entities.contains_key(to) {
            return None;
        }

        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(from.to_string());

        let mut queue: VecDeque<Vec<String>> = VecDeque::new();
        queue.push_back(vec![from.to_string()]);

        while let Some(path) = queue.pop_front() {
            let Some(last) = path.last() else {
                continue;
            };
            let current = last.as_str();

            let neighbor_ids = self.adjacent_ids(current);
            for neighbor in neighbor_ids {
                if neighbor == to {
                    let mut full = path.clone();
                    full.push(neighbor);
                    return Some(full);
                }
                if visited.insert(neighbor.clone()) {
                    let mut new_path = path.clone();
                    new_path.push(neighbor);
                    queue.push_back(new_path);
                }
            }
        }

        None
    }

    /// Return all entities in the connected component containing `entity_id`.
    /// Treats the graph as undirected.
    pub fn connected_component(&self, entity_id: &str) -> Vec<&Entity> {
        if !self.entities.contains_key(entity_id) {
            return Vec::new();
        }

        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(entity_id);

        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(entity_id);

        let mut result = Vec::new();

        // Include the starting entity itself
        if let Some(e) = self.entities.get(entity_id) {
            result.push(e);
        }

        while let Some(current) = queue.pop_front() {
            // Outgoing
            if let Some(idxs) = self.relations_from.get(current) {
                for &idx in idxs {
                    if let Some(rel) = self.relationships.get(idx) {
                        let neighbor = rel.to_entity.as_str();
                        if visited.insert(neighbor) {
                            if let Some(entity) = self.entities.get(neighbor) {
                                result.push(entity);
                                queue.push_back(neighbor);
                            }
                        }
                    }
                }
            }

            // Incoming
            if let Some(idxs) = self.relations_to.get(current) {
                for &idx in idxs {
                    if let Some(rel) = self.relationships.get(idx) {
                        let neighbor = rel.from_entity.as_str();
                        if visited.insert(neighbor) {
                            if let Some(entity) = self.entities.get(neighbor) {
                                result.push(entity);
                                queue.push_back(neighbor);
                            }
                        }
                    }
                }
            }
        }

        result
    }

    /// Total number of entities.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Total number of relationships.
    pub fn relationship_count(&self) -> usize {
        self.relationships.len()
    }

    // -----------------------------------------------------------------------
    // Knowledge extraction
    // -----------------------------------------------------------------------

    /// Extract entities from free text using regex-based heuristics.
    ///
    /// Detects:
    /// - Capitalized multi-word phrases (potential names / organizations)
    /// - Email addresses (creates Person entities)
    /// - URLs (creates Custom("Url") entities)
    /// - @mentions (creates Person entities)
    /// - #hashtags (creates Concept entities)
    /// - File paths (creates File entities)
    ///
    /// Returns the IDs of all newly created entities.
    pub fn extract_entities_from_text(&mut self, text: &str, source: &str) -> Vec<String> {
        let mut ids = Vec::new();
        let now = Utc::now();

        // All patterns are compile-time constant strings; return empty if any
        // somehow fails (should never happen).
        let Ok(email_re) = Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}") else {
            return ids;
        };
        let Ok(url_re) = Regex::new(r"https?://[a-zA-Z0-9._~:/?#\[\]@!$&'()*+,;=%-]+") else {
            return ids;
        };
        let Ok(mention_re) = Regex::new(r"@([a-zA-Z0-9_]+)") else {
            return ids;
        };
        let Ok(hashtag_re) = Regex::new(r"#([a-zA-Z0-9_]+)") else {
            return ids;
        };
        let Ok(filepath_re) = Regex::new(r"(?:^|[\s(])(/[a-zA-Z0-9._/-]+\.[a-zA-Z0-9]+)") else {
            return ids;
        };
        let Ok(cap_phrase_re) = Regex::new(r"\b([A-Z][a-z]+(?:\s+[A-Z][a-z]+)+)\b") else {
            return ids;
        };

        // Track what we already extracted to avoid duplicates within one call
        let mut seen: HashSet<String> = HashSet::new();

        // Emails -> Person
        for m in email_re.find_iter(text) {
            let email = m.as_str().to_string();
            if seen.insert(email.clone()) {
                let name = email.split('@').next().unwrap_or(&email).to_string();
                let entity = Entity {
                    id: String::new(),
                    name: name.clone(),
                    entity_type: EntityType::Person,
                    properties: HashMap::from([(
                        "email".to_string(),
                        serde_json::Value::String(email),
                    )]),
                    created_at: now,
                    updated_at: now,
                    confidence: 0.8,
                    source: source.to_string(),
                };
                ids.push(self.add_entity(entity));
            }
        }

        // URLs -> Custom("Url")
        for m in url_re.find_iter(text) {
            let url = m.as_str().to_string();
            if seen.insert(url.clone()) {
                let entity = Entity {
                    id: String::new(),
                    name: url.clone(),
                    entity_type: EntityType::Custom("Url".to_string()),
                    properties: HashMap::from([(
                        "url".to_string(),
                        serde_json::Value::String(url),
                    )]),
                    created_at: now,
                    updated_at: now,
                    confidence: 0.9,
                    source: source.to_string(),
                };
                ids.push(self.add_entity(entity));
            }
        }

        // @mentions -> Person
        for cap in mention_re.captures_iter(text) {
            let username = cap[1].to_string();
            let key = format!("@{username}");
            if seen.insert(key) {
                let entity = Entity {
                    id: String::new(),
                    name: username.clone(),
                    entity_type: EntityType::Person,
                    properties: HashMap::from([(
                        "mention".to_string(),
                        serde_json::Value::String(format!("@{username}")),
                    )]),
                    created_at: now,
                    updated_at: now,
                    confidence: 0.7,
                    source: source.to_string(),
                };
                ids.push(self.add_entity(entity));
            }
        }

        // #hashtags -> Concept
        for cap in hashtag_re.captures_iter(text) {
            let tag = cap[1].to_string();
            let key = format!("#{tag}");
            if seen.insert(key) {
                let entity = Entity {
                    id: String::new(),
                    name: tag.clone(),
                    entity_type: EntityType::Concept,
                    properties: HashMap::from([(
                        "hashtag".to_string(),
                        serde_json::Value::String(format!("#{tag}")),
                    )]),
                    created_at: now,
                    updated_at: now,
                    confidence: 0.8,
                    source: source.to_string(),
                };
                ids.push(self.add_entity(entity));
            }
        }

        // File paths -> File
        for cap in filepath_re.captures_iter(text) {
            let path = cap[1].to_string();
            if seen.insert(path.clone()) {
                let file_name = path.rsplit('/').next().unwrap_or(&path).to_string();
                let entity = Entity {
                    id: String::new(),
                    name: file_name,
                    entity_type: EntityType::File,
                    properties: HashMap::from([(
                        "path".to_string(),
                        serde_json::Value::String(path),
                    )]),
                    created_at: now,
                    updated_at: now,
                    confidence: 0.85,
                    source: source.to_string(),
                };
                ids.push(self.add_entity(entity));
            }
        }

        // Capitalized phrases -> Person (heuristic: multi-word capitalized = name/org)
        for cap in cap_phrase_re.captures_iter(text) {
            let phrase = cap[1].to_string();
            if seen.insert(phrase.clone()) {
                // Skip if it is just two very common words
                let entity = Entity {
                    id: String::new(),
                    name: phrase,
                    entity_type: EntityType::Person,
                    properties: HashMap::new(),
                    created_at: now,
                    updated_at: now,
                    confidence: 0.5,
                    source: source.to_string(),
                };
                ids.push(self.add_entity(entity));
            }
        }

        ids
    }

    // -----------------------------------------------------------------------
    // Entity merge (deduplication)
    // -----------------------------------------------------------------------

    /// Merge `source_id` entity into `target_id`.
    ///
    /// All properties from the source are copied to the target (existing keys are
    /// not overwritten). All relationships that reference the source entity are
    /// redirected to the target. The source entity is then removed.
    pub fn merge_entity(&mut self, source_id: &str, target_id: &str) -> ArgentorResult<()> {
        if source_id == target_id {
            return Err(ArgentorError::Agent(
                "Cannot merge an entity with itself".to_string(),
            ));
        }

        let source = self
            .entities
            .get(source_id)
            .ok_or_else(|| ArgentorError::Agent(format!("Source entity '{source_id}' not found")))?
            .clone();

        let target = self.entities.get(target_id).ok_or_else(|| {
            ArgentorError::Agent(format!("Target entity '{target_id}' not found"))
        })?;

        // Merge properties (source into target, don't overwrite existing)
        let mut merged_props = target.properties.clone();
        for (k, v) in &source.properties {
            merged_props.entry(k.clone()).or_insert_with(|| v.clone());
        }

        if let Some(target_mut) = self.entities.get_mut(target_id) {
            target_mut.properties = merged_props;
            target_mut.updated_at = Utc::now();
        }

        // Redirect relationships
        for rel in &mut self.relationships {
            if rel.from_entity == source_id {
                rel.from_entity = target_id.to_string();
            }
            if rel.to_entity == source_id {
                rel.to_entity = target_id.to_string();
            }
        }

        // Remove self-loops that may have been created by the merge
        self.relationships
            .retain(|r| !(r.from_entity == r.to_entity && r.from_entity == target_id));

        // Remove source entity from maps (not using remove_entity to avoid
        // double-removing relationships we already redirected).
        self.entities.remove(source_id);

        // Remove from name index
        let name_lower = source.name.to_lowercase();
        if let Some(ids) = self.entity_by_name.get_mut(&name_lower) {
            ids.retain(|i| i != source_id);
            if ids.is_empty() {
                self.entity_by_name.remove(&name_lower);
            }
        }

        // Remove from type index
        if let Some(ids) = self.entity_by_type.get_mut(&source.entity_type) {
            ids.retain(|i| i != source_id);
            if ids.is_empty() {
                self.entity_by_type.remove(&source.entity_type);
            }
        }

        // Rebuild relation indexes since we mutated from/to
        self.rebuild_relation_indexes();

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Summary & context
    // -----------------------------------------------------------------------

    /// Compute aggregate statistics about the graph.
    pub fn summarize(&self) -> GraphSummary {
        let mut entity_types: HashMap<String, usize> = HashMap::new();
        for entity in self.entities.values() {
            *entity_types
                .entry(entity.entity_type.to_string())
                .or_default() += 1;
        }

        let mut relationship_types: HashMap<String, usize> = HashMap::new();
        for rel in &self.relationships {
            *relationship_types
                .entry(rel.relation_type.to_string())
                .or_default() += 1;
        }

        // Connection count per entity (outgoing + incoming)
        let mut connection_count: HashMap<&str, usize> = HashMap::new();
        for rel in &self.relationships {
            *connection_count.entry(&rel.from_entity).or_default() += 1;
            *connection_count.entry(&rel.to_entity).or_default() += 1;
        }

        let mut most_connected: Vec<(String, usize)> = connection_count
            .into_iter()
            .filter_map(|(id, count)| self.entities.get(id).map(|e| (e.name.clone(), count)))
            .collect();
        most_connected.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        most_connected.truncate(10);

        GraphSummary {
            entity_count: self.entities.len(),
            relationship_count: self.relationships.len(),
            entity_types,
            relationship_types,
            most_connected,
        }
    }

    /// Generate a human-readable context string about an entity and its neighborhood.
    ///
    /// Output includes entity properties, direct relationships, and optionally
    /// relationships of neighbors up to the given depth.
    pub fn to_context_string(&self, entity_id: &str, depth: usize) -> String {
        let entity = match self.entities.get(entity_id) {
            Some(e) => e,
            None => return format!("Entity '{entity_id}' not found."),
        };

        let mut out = String::new();

        // Entity header
        out.push_str(&format!(
            "Entity: {} ({})\n",
            entity.name, entity.entity_type
        ));

        // Properties
        if !entity.properties.is_empty() {
            let props: Vec<String> = entity
                .properties
                .iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    format!("{k}={val}")
                })
                .collect();
            out.push_str(&format!("  Properties: {}\n", props.join(", ")));
        }

        // Direct relationships (outgoing)
        let outgoing = self.get_relationships_from(entity_id);
        let incoming = self.get_relationships_to(entity_id);

        if !outgoing.is_empty() || !incoming.is_empty() {
            out.push_str("  Relationships:\n");
            for rel in &outgoing {
                let target_name = self
                    .entities
                    .get(&rel.to_entity)
                    .map(|e| format!("{} ({})", e.name, e.entity_type))
                    .unwrap_or_else(|| rel.to_entity.clone());
                out.push_str(&format!(
                    "    -> {} -> {}\n",
                    rel.relation_type, target_name
                ));
            }
            for rel in &incoming {
                let source_name = self
                    .entities
                    .get(&rel.from_entity)
                    .map(|e| format!("{} ({})", e.name, e.entity_type))
                    .unwrap_or_else(|| rel.from_entity.clone());
                out.push_str(&format!(
                    "    <- {} <- {}\n",
                    rel.relation_type, source_name
                ));
            }
        }

        // Neighbor relationships at deeper levels
        if depth > 1 {
            let neighbors = self.neighbors(entity_id, depth);
            if !neighbors.is_empty() {
                out.push_str(&format!("  Neighbors (depth {depth}):\n"));
                for neighbor in &neighbors {
                    let n_out = self.get_relationships_from(&neighbor.id);
                    for rel in n_out {
                        if rel.to_entity == entity_id {
                            continue; // skip back-link to origin
                        }
                        let target_name = self
                            .entities
                            .get(&rel.to_entity)
                            .map(|e| e.name.as_str())
                            .unwrap_or("?");
                        out.push_str(&format!(
                            "    {} -> {} -> {}\n",
                            neighbor.name, rel.relation_type, target_name
                        ));
                    }
                }
            }
        }

        out
    }

    // -----------------------------------------------------------------------
    // Persistence
    // -----------------------------------------------------------------------

    /// Save the graph to a JSON file.
    pub fn save(&self, path: &Path) -> ArgentorResult<()> {
        let data = SerializableGraph {
            entities: self.entities.values().cloned().collect(),
            relationships: self.relationships.clone(),
        };
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| ArgentorError::Session(format!("Failed to serialize graph: {e}")))?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ArgentorError::Session(format!("Failed to create dir: {e}")))?;
        }
        std::fs::write(path, json)
            .map_err(|e| ArgentorError::Session(format!("Failed to write graph: {e}")))?;
        Ok(())
    }

    /// Load a graph from a JSON file.
    pub fn load(path: &Path) -> ArgentorResult<Self> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| ArgentorError::Session(format!("Failed to read graph: {e}")))?;
        let sg: SerializableGraph = serde_json::from_str(&data)
            .map_err(|e| ArgentorError::Session(format!("Failed to deserialize graph: {e}")))?;

        let mut graph = Self::new();
        for entity in sg.entities {
            graph.add_entity(entity);
        }
        for rel in sg.relationships {
            graph.add_relationship(rel);
        }
        Ok(graph)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Collect IDs of all adjacent entities (both directions).
    fn adjacent_ids(&self, entity_id: &str) -> Vec<String> {
        let mut result = Vec::new();
        if let Some(idxs) = self.relations_from.get(entity_id) {
            for &idx in idxs {
                if let Some(rel) = self.relationships.get(idx) {
                    result.push(rel.to_entity.clone());
                }
            }
        }
        if let Some(idxs) = self.relations_to.get(entity_id) {
            for &idx in idxs {
                if let Some(rel) = self.relationships.get(idx) {
                    result.push(rel.from_entity.clone());
                }
            }
        }
        result
    }

    /// Rebuild the from/to relationship index maps from scratch.
    fn rebuild_relation_indexes(&mut self) {
        self.relations_from.clear();
        self.relations_to.clear();
        for (idx, rel) in self.relationships.iter().enumerate() {
            self.relations_from
                .entry(rel.from_entity.clone())
                .or_default()
                .push(idx);
            self.relations_to
                .entry(rel.to_entity.clone())
                .or_default()
                .push(idx);
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // Helpers ----------------------------------------------------------------

    fn make_entity(name: &str, etype: EntityType) -> Entity {
        Entity {
            id: String::new(),
            name: name.to_string(),
            entity_type: etype,
            properties: HashMap::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            confidence: 1.0,
            source: "test".to_string(),
        }
    }

    fn make_entity_with_props(
        name: &str,
        etype: EntityType,
        props: HashMap<String, serde_json::Value>,
    ) -> Entity {
        Entity {
            id: String::new(),
            name: name.to_string(),
            entity_type: etype,
            properties: props,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            confidence: 1.0,
            source: "test".to_string(),
        }
    }

    fn make_rel(from: &str, to: &str, rtype: RelationType) -> Relationship {
        Relationship {
            id: String::new(),
            from_entity: from.to_string(),
            to_entity: to.to_string(),
            relation_type: rtype,
            properties: HashMap::new(),
            weight: 1.0,
            created_at: Utc::now(),
            source: "test".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Entity CRUD tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_add_entity_generates_id() {
        let mut graph = KnowledgeGraph::new();
        let id = graph.add_entity(make_entity("Alice", EntityType::Person));
        assert!(!id.is_empty());
        assert_eq!(graph.entity_count(), 1);
    }

    #[test]
    fn test_add_entity_custom_id() {
        let mut graph = KnowledgeGraph::new();
        let mut e = make_entity("Bob", EntityType::Person);
        e.id = "custom-id".to_string();
        let id = graph.add_entity(e);
        assert_eq!(id, "custom-id");
    }

    #[test]
    fn test_get_entity() {
        let mut graph = KnowledgeGraph::new();
        let id = graph.add_entity(make_entity("Alice", EntityType::Person));
        let entity = graph.get_entity(&id).unwrap();
        assert_eq!(entity.name, "Alice");
    }

    #[test]
    fn test_get_entity_not_found() {
        let graph = KnowledgeGraph::new();
        assert!(graph.get_entity("nonexistent").is_none());
    }

    #[test]
    fn test_find_entities_by_name() {
        let mut graph = KnowledgeGraph::new();
        graph.add_entity(make_entity("Alice Smith", EntityType::Person));
        graph.add_entity(make_entity("Bob Jones", EntityType::Person));
        graph.add_entity(make_entity("Alice Cooper", EntityType::Person));

        let results = graph.find_entities("alice");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_find_entities_case_insensitive() {
        let mut graph = KnowledgeGraph::new();
        graph.add_entity(make_entity("Rust Language", EntityType::Concept));

        let results = graph.find_entities("RUST");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Rust Language");
    }

    #[test]
    fn test_find_entities_no_match() {
        let mut graph = KnowledgeGraph::new();
        graph.add_entity(make_entity("Alice", EntityType::Person));

        let results = graph.find_entities("zzz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_find_by_type() {
        let mut graph = KnowledgeGraph::new();
        graph.add_entity(make_entity("Alice", EntityType::Person));
        graph.add_entity(make_entity("Rust", EntityType::Concept));
        graph.add_entity(make_entity("Bob", EntityType::Person));

        let people = graph.find_by_type(&EntityType::Person);
        assert_eq!(people.len(), 2);

        let concepts = graph.find_by_type(&EntityType::Concept);
        assert_eq!(concepts.len(), 1);

        let tools = graph.find_by_type(&EntityType::Tool);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_update_entity() {
        let mut graph = KnowledgeGraph::new();
        let id = graph.add_entity(make_entity("Alice", EntityType::Person));

        let mut props = HashMap::new();
        props.insert("role".to_string(), serde_json::json!("engineer"));
        assert!(graph.update_entity(&id, props));

        let updated = graph.get_entity(&id).unwrap();
        assert_eq!(
            updated.properties.get("role").unwrap(),
            &serde_json::json!("engineer")
        );
    }

    #[test]
    fn test_update_entity_not_found() {
        let mut graph = KnowledgeGraph::new();
        assert!(!graph.update_entity("nonexistent", HashMap::new()));
    }

    #[test]
    fn test_update_entity_preserves_existing_props() {
        let mut graph = KnowledgeGraph::new();
        let props = HashMap::from([("email".to_string(), serde_json::json!("a@b.com"))]);
        let id = graph.add_entity(make_entity_with_props("Alice", EntityType::Person, props));

        let new_props = HashMap::from([("role".to_string(), serde_json::json!("lead"))]);
        graph.update_entity(&id, new_props);

        let e = graph.get_entity(&id).unwrap();
        assert!(e.properties.contains_key("email"));
        assert!(e.properties.contains_key("role"));
    }

    #[test]
    fn test_remove_entity() {
        let mut graph = KnowledgeGraph::new();
        let id = graph.add_entity(make_entity("Alice", EntityType::Person));
        assert!(graph.remove_entity(&id));
        assert_eq!(graph.entity_count(), 0);
        assert!(graph.get_entity(&id).is_none());
    }

    #[test]
    fn test_remove_entity_not_found() {
        let mut graph = KnowledgeGraph::new();
        assert!(!graph.remove_entity("nonexistent"));
    }

    #[test]
    fn test_remove_entity_cascades_relationships() {
        let mut graph = KnowledgeGraph::new();
        let alice = graph.add_entity(make_entity("Alice", EntityType::Person));
        let bob = graph.add_entity(make_entity("Bob", EntityType::Person));
        graph.add_relationship(make_rel(&alice, &bob, RelationType::WorksWith));
        assert_eq!(graph.relationship_count(), 1);

        graph.remove_entity(&alice);
        assert_eq!(graph.relationship_count(), 0);
        assert_eq!(graph.entity_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Relationship CRUD tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_add_relationship() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let rel_id = graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        assert!(!rel_id.is_empty());
        assert_eq!(graph.relationship_count(), 1);
    }

    #[test]
    fn test_get_relationships_from() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let c = graph.add_entity(make_entity("C", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&a, &c, RelationType::DependsOn));

        let from_a = graph.get_relationships_from(&a);
        assert_eq!(from_a.len(), 2);
    }

    #[test]
    fn test_get_relationships_to() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let c = graph.add_entity(make_entity("C", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &c, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&b, &c, RelationType::DependsOn));

        let to_c = graph.get_relationships_to(&c);
        assert_eq!(to_c.len(), 2);
    }

    #[test]
    fn test_find_relationships_by_from() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&b, &a, RelationType::DependsOn));

        let rels = graph.find_relationships(Some(&a), None, None);
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].from_entity, a);
    }

    #[test]
    fn test_find_relationships_by_type() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&a, &b, RelationType::DependsOn));

        let rels = graph.find_relationships(None, None, Some(&RelationType::DependsOn));
        assert_eq!(rels.len(), 1);
    }

    #[test]
    fn test_find_relationships_combined_filters() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let c = graph.add_entity(make_entity("C", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&a, &c, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&a, &b, RelationType::DependsOn));

        let rels = graph.find_relationships(Some(&a), Some(&b), Some(&RelationType::RelatedTo));
        assert_eq!(rels.len(), 1);
    }

    #[test]
    fn test_remove_relationship() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let rel_id = graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        assert!(graph.remove_relationship(&rel_id));
        assert_eq!(graph.relationship_count(), 0);
    }

    #[test]
    fn test_remove_relationship_not_found() {
        let mut graph = KnowledgeGraph::new();
        assert!(!graph.remove_relationship("nonexistent"));
    }

    // -----------------------------------------------------------------------
    // Graph query tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_neighbors_depth_1() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let c = graph.add_entity(make_entity("C", EntityType::Concept));
        let d = graph.add_entity(make_entity("D", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&a, &c, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&b, &d, RelationType::RelatedTo));

        let neighbors = graph.neighbors(&a, 1);
        assert_eq!(neighbors.len(), 2); // B and C only
        let names: HashSet<&str> = neighbors.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains("B"));
        assert!(names.contains("C"));
        assert!(!names.contains("D")); // depth 2
    }

    #[test]
    fn test_neighbors_depth_2() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let c = graph.add_entity(make_entity("C", EntityType::Concept));
        let d = graph.add_entity(make_entity("D", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&b, &c, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&c, &d, RelationType::RelatedTo));

        let neighbors = graph.neighbors(&a, 2);
        assert_eq!(neighbors.len(), 2); // B (depth 1) and C (depth 2)
        let names: HashSet<&str> = neighbors.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains("B"));
        assert!(names.contains("C"));
        assert!(!names.contains("D")); // depth 3
    }

    #[test]
    fn test_neighbors_nonexistent_entity() {
        let graph = KnowledgeGraph::new();
        let neighbors = graph.neighbors("nope", 1);
        assert!(neighbors.is_empty());
    }

    #[test]
    fn test_neighbors_zero_depth() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));

        let neighbors = graph.neighbors(&a, 0);
        assert!(neighbors.is_empty());
    }

    #[test]
    fn test_neighbors_undirected() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        // Only B->A edge, but neighbors should still find B from A
        graph.add_relationship(make_rel(&b, &a, RelationType::RelatedTo));

        let neighbors = graph.neighbors(&a, 1);
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].name, "B");
    }

    #[test]
    fn test_shortest_path_direct() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));

        let path = graph.shortest_path(&a, &b).unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0], a);
        assert_eq!(path[1], b);
    }

    #[test]
    fn test_shortest_path_multi_hop() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let c = graph.add_entity(make_entity("C", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&b, &c, RelationType::RelatedTo));

        let path = graph.shortest_path(&a, &c).unwrap();
        assert_eq!(path.len(), 3);
        assert_eq!(path, vec![a, b, c]);
    }

    #[test]
    fn test_shortest_path_same_node() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));

        let path = graph.shortest_path(&a, &a).unwrap();
        assert_eq!(path.len(), 1);
    }

    #[test]
    fn test_shortest_path_no_path() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let _b = graph.add_entity(make_entity("B", EntityType::Concept));
        // No edge between A and B
        assert!(graph.shortest_path(&a, &_b).is_none());
    }

    #[test]
    fn test_shortest_path_nonexistent() {
        let graph = KnowledgeGraph::new();
        assert!(graph.shortest_path("x", "y").is_none());
    }

    #[test]
    fn test_connected_component() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let c = graph.add_entity(make_entity("C", EntityType::Concept));
        let d = graph.add_entity(make_entity("D", EntityType::Concept)); // isolated
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&b, &c, RelationType::RelatedTo));

        let comp = graph.connected_component(&a);
        assert_eq!(comp.len(), 3); // A, B, C
        let names: HashSet<&str> = comp.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains("A"));
        assert!(names.contains("B"));
        assert!(names.contains("C"));
        assert!(!names.contains("D"));

        let comp_d = graph.connected_component(&d);
        assert_eq!(comp_d.len(), 1); // just D itself
    }

    #[test]
    fn test_connected_component_nonexistent() {
        let graph = KnowledgeGraph::new();
        assert!(graph.connected_component("nope").is_empty());
    }

    #[test]
    fn test_entity_count_and_relationship_count() {
        let mut graph = KnowledgeGraph::new();
        assert_eq!(graph.entity_count(), 0);
        assert_eq!(graph.relationship_count(), 0);

        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));

        assert_eq!(graph.entity_count(), 2);
        assert_eq!(graph.relationship_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Entity extraction tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_emails() {
        let mut graph = KnowledgeGraph::new();
        let ids = graph.extract_entities_from_text("Contact alice@example.com for details", "test");
        assert!(!ids.is_empty());
        let entity = graph.get_entity(&ids[0]).unwrap();
        assert_eq!(entity.entity_type, EntityType::Person);
        assert_eq!(
            entity.properties.get("email").unwrap(),
            &serde_json::json!("alice@example.com")
        );
    }

    #[test]
    fn test_extract_urls() {
        let mut graph = KnowledgeGraph::new();
        let ids = graph.extract_entities_from_text(
            "Check https://github.com/fboiero/Argentor for source",
            "test",
        );
        let url_entities: Vec<&Entity> = ids
            .iter()
            .filter_map(|id| graph.get_entity(id))
            .filter(|e| e.entity_type == EntityType::Custom("Url".to_string()))
            .collect();
        assert_eq!(url_entities.len(), 1);
    }

    #[test]
    fn test_extract_mentions() {
        let mut graph = KnowledgeGraph::new();
        let ids = graph.extract_entities_from_text("Thanks @johndoe and @janedoe", "test");
        let mention_entities: Vec<&Entity> = ids
            .iter()
            .filter_map(|id| graph.get_entity(id))
            .filter(|e| e.properties.contains_key("mention"))
            .collect();
        assert_eq!(mention_entities.len(), 2);
    }

    #[test]
    fn test_extract_hashtags() {
        let mut graph = KnowledgeGraph::new();
        let ids = graph.extract_entities_from_text("Discussing #rust and #wasm today", "test");
        let tag_entities: Vec<&Entity> = ids
            .iter()
            .filter_map(|id| graph.get_entity(id))
            .filter(|e| e.entity_type == EntityType::Concept)
            .collect();
        assert_eq!(tag_entities.len(), 2);
    }

    #[test]
    fn test_extract_file_paths() {
        let mut graph = KnowledgeGraph::new();
        let ids =
            graph.extract_entities_from_text("Edit the file /src/main.rs to fix the bug", "test");
        let file_entities: Vec<&Entity> = ids
            .iter()
            .filter_map(|id| graph.get_entity(id))
            .filter(|e| e.entity_type == EntityType::File)
            .collect();
        assert_eq!(file_entities.len(), 1);
        assert_eq!(file_entities[0].name, "main.rs");
    }

    #[test]
    fn test_extract_capitalized_phrases() {
        let mut graph = KnowledgeGraph::new();
        let ids = graph.extract_entities_from_text(
            "I met Alice Cooper at the event and saw Bob Dylan too",
            "test",
        );
        let person_entities: Vec<&Entity> = ids
            .iter()
            .filter_map(|id| graph.get_entity(id))
            .filter(|e| e.entity_type == EntityType::Person && e.confidence == 0.5)
            .collect();
        assert!(person_entities.len() >= 2);
        let names: HashSet<&str> = person_entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains("Alice Cooper"));
        assert!(names.contains("Bob Dylan"));
    }

    #[test]
    fn test_extract_no_duplicates() {
        let mut graph = KnowledgeGraph::new();
        let ids = graph.extract_entities_from_text(
            "Contact alice@example.com and alice@example.com again",
            "test",
        );
        let email_entities: Vec<&Entity> = ids
            .iter()
            .filter_map(|id| graph.get_entity(id))
            .filter(|e| e.properties.contains_key("email"))
            .collect();
        assert_eq!(email_entities.len(), 1);
    }

    #[test]
    fn test_extract_empty_text() {
        let mut graph = KnowledgeGraph::new();
        let ids = graph.extract_entities_from_text("", "test");
        assert!(ids.is_empty());
    }

    // -----------------------------------------------------------------------
    // Entity merge tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_merge_entity_basic() {
        let mut graph = KnowledgeGraph::new();
        let props_a = HashMap::from([("email".to_string(), serde_json::json!("a@x.com"))]);
        let props_b = HashMap::from([("role".to_string(), serde_json::json!("dev"))]);
        let a = graph.add_entity(make_entity_with_props("Alice", EntityType::Person, props_a));
        let b = graph.add_entity(make_entity_with_props(
            "Alice Dup",
            EntityType::Person,
            props_b,
        ));

        graph.merge_entity(&b, &a).unwrap();
        assert_eq!(graph.entity_count(), 1);

        let merged = graph.get_entity(&a).unwrap();
        assert!(merged.properties.contains_key("email"));
        assert!(merged.properties.contains_key("role"));
    }

    #[test]
    fn test_merge_entity_redirects_relationships() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("Alice", EntityType::Person));
        let dup = graph.add_entity(make_entity("Alice Dup", EntityType::Person));
        let bob = graph.add_entity(make_entity("Bob", EntityType::Person));
        graph.add_relationship(make_rel(&dup, &bob, RelationType::WorksWith));

        graph.merge_entity(&dup, &a).unwrap();

        let rels = graph.get_relationships_from(&a);
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].to_entity, bob);
    }

    #[test]
    fn test_merge_entity_self_error() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        assert!(graph.merge_entity(&a, &a).is_err());
    }

    #[test]
    fn test_merge_entity_not_found() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Concept));
        assert!(graph.merge_entity("nonexistent", &a).is_err());
        assert!(graph.merge_entity(&a, "nonexistent").is_err());
    }

    #[test]
    fn test_merge_entity_no_overwrite_existing() {
        let mut graph = KnowledgeGraph::new();
        let props_a = HashMap::from([("email".to_string(), serde_json::json!("target@x.com"))]);
        let props_b = HashMap::from([("email".to_string(), serde_json::json!("source@x.com"))]);
        let a = graph.add_entity(make_entity_with_props("A", EntityType::Person, props_a));
        let b = graph.add_entity(make_entity_with_props("B", EntityType::Person, props_b));

        graph.merge_entity(&b, &a).unwrap();
        let merged = graph.get_entity(&a).unwrap();
        // Target's email should be preserved (not overwritten)
        assert_eq!(
            merged.properties.get("email").unwrap(),
            &serde_json::json!("target@x.com")
        );
    }

    // -----------------------------------------------------------------------
    // Context string tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_to_context_string_basic() {
        let mut graph = KnowledgeGraph::new();
        let props = HashMap::from([
            ("email".to_string(), serde_json::json!("alice@example.com")),
            ("role".to_string(), serde_json::json!("engineer")),
        ]);
        let alice = graph.add_entity(make_entity_with_props("Alice", EntityType::Person, props));
        let bob = graph.add_entity(make_entity("Bob", EntityType::Person));
        let rust = graph.add_entity(make_entity("Rust", EntityType::Concept));
        graph.add_relationship(make_rel(&alice, &bob, RelationType::WorksWith));
        graph.add_relationship(make_rel(&alice, &rust, RelationType::Mentions));

        let ctx = graph.to_context_string(&alice, 1);
        assert!(ctx.contains("Entity: Alice (Person)"));
        assert!(ctx.contains("Properties:"));
        assert!(ctx.contains("email=alice@example.com"));
        assert!(ctx.contains("Relationships:"));
        assert!(ctx.contains("WorksWith"));
        assert!(ctx.contains("Bob"));
        assert!(ctx.contains("Mentions"));
        assert!(ctx.contains("Rust"));
    }

    #[test]
    fn test_to_context_string_not_found() {
        let graph = KnowledgeGraph::new();
        let ctx = graph.to_context_string("nonexistent", 1);
        assert!(ctx.contains("not found"));
    }

    #[test]
    fn test_to_context_string_depth_2() {
        let mut graph = KnowledgeGraph::new();
        let alice = graph.add_entity(make_entity("Alice", EntityType::Person));
        let bob = graph.add_entity(make_entity("Bob", EntityType::Person));
        let charlie = graph.add_entity(make_entity("Charlie", EntityType::Person));
        graph.add_relationship(make_rel(&alice, &bob, RelationType::WorksWith));
        graph.add_relationship(make_rel(&bob, &charlie, RelationType::WorksWith));

        let ctx = graph.to_context_string(&alice, 2);
        assert!(ctx.contains("Neighbors (depth 2)"));
        assert!(ctx.contains("Charlie"));
    }

    // -----------------------------------------------------------------------
    // Summary tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_summarize_empty() {
        let graph = KnowledgeGraph::new();
        let summary = graph.summarize();
        assert_eq!(summary.entity_count, 0);
        assert_eq!(summary.relationship_count, 0);
        assert!(summary.most_connected.is_empty());
    }

    #[test]
    fn test_summarize_populated() {
        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("A", EntityType::Person));
        let b = graph.add_entity(make_entity("B", EntityType::Concept));
        let c = graph.add_entity(make_entity("C", EntityType::Person));
        graph.add_relationship(make_rel(&a, &b, RelationType::RelatedTo));
        graph.add_relationship(make_rel(&a, &c, RelationType::WorksWith));
        graph.add_relationship(make_rel(&b, &c, RelationType::Mentions));

        let summary = graph.summarize();
        assert_eq!(summary.entity_count, 3);
        assert_eq!(summary.relationship_count, 3);
        assert_eq!(summary.entity_types.get("Person"), Some(&2));
        assert_eq!(summary.entity_types.get("Concept"), Some(&1));
        assert_eq!(summary.relationship_types.get("RelatedTo"), Some(&1));
        assert_eq!(summary.relationship_types.get("WorksWith"), Some(&1));

        // A has 2 outgoing rels, B has 1 outgoing + 1 incoming = 2, C has 2 incoming
        // All have count 2, so all three should be in most_connected
        assert!(!summary.most_connected.is_empty());
    }

    // -----------------------------------------------------------------------
    // Persistence tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("graph.json");

        let mut graph = KnowledgeGraph::new();
        let a = graph.add_entity(make_entity("Alice", EntityType::Person));
        let b = graph.add_entity(make_entity("Bob", EntityType::Person));
        let rel_id = graph.add_relationship(make_rel(&a, &b, RelationType::WorksWith));

        graph.save(&path).unwrap();

        let loaded = KnowledgeGraph::load(&path).unwrap();
        assert_eq!(loaded.entity_count(), 2);
        assert_eq!(loaded.relationship_count(), 1);

        let loaded_alice = loaded.find_entities("alice");
        assert_eq!(loaded_alice.len(), 1);
        assert_eq!(loaded_alice[0].name, "Alice");

        let loaded_rels = loaded.find_relationships(Some(&a), Some(&b), None);
        assert_eq!(loaded_rels.len(), 1);
        assert_eq!(loaded_rels[0].id, rel_id);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = KnowledgeGraph::load(Path::new("/tmp/does_not_exist_kg.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("dir").join("graph.json");

        let graph = KnowledgeGraph::new();
        graph.save(&path).unwrap();
        assert!(path.exists());
    }

    // -----------------------------------------------------------------------
    // Default trait
    // -----------------------------------------------------------------------

    #[test]
    fn test_default_graph() {
        let graph = KnowledgeGraph::default();
        assert_eq!(graph.entity_count(), 0);
        assert_eq!(graph.relationship_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Display trait tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_entity_type_display() {
        assert_eq!(EntityType::Person.to_string(), "Person");
        assert_eq!(EntityType::Custom("X".to_string()).to_string(), "Custom(X)");
    }

    #[test]
    fn test_relation_type_display() {
        assert_eq!(RelationType::IsA.to_string(), "IsA");
        assert_eq!(
            RelationType::Custom("Likes".to_string()).to_string(),
            "Custom(Likes)"
        );
    }
}
