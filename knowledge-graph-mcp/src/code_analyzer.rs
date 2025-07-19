use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, Query, QueryCursor, Node, Language};
use walkdir::WalkDir;
use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::sync::mpsc;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug, warn};
use uuid::Uuid;

use crate::knowledge_graph::{CodeEntity, EntityType, RelationshipType};

pub struct CodeAnalyzer {
    parsers: Arc<RwLock<LanguageParsers>>,
    file_watcher: Option<notify::RecommendedWatcher>,
}

struct LanguageParsers {
    rust: Parser,
    javascript: Parser,
    python: Parser,
}

impl LanguageParsers {
    fn new() -> Result<Self> {
        let mut rust = Parser::new();
        rust.set_language(tree_sitter_rust::language())
            .map_err(|e| anyhow!("Failed to set Rust language: {}", e))?;
        
        let mut javascript = Parser::new();
        javascript.set_language(tree_sitter_javascript::language())
            .map_err(|e| anyhow!("Failed to set JavaScript language: {}", e))?;
        
        let mut python = Parser::new();
        python.set_language(tree_sitter_python::language())
            .map_err(|e| anyhow!("Failed to set Python language: {}", e))?;
        
        Ok(Self {
            rust,
            javascript,
            python,
        })
    }
}

impl CodeAnalyzer {
    pub fn new() -> Self {
        let parsers = LanguageParsers::new()
            .expect("Failed to initialize language parsers");
        
        Self {
            parsers: Arc::new(RwLock::new(parsers)),
            file_watcher: None,
        }
    }
    
    pub async fn analyze_project(&self, project_path: &Path) -> Result<Vec<CodeEntity>> {
        info!("Analyzing project: {:?}", project_path);
        
        let mut entities = Vec::new();
        let supported_extensions = vec!["rs", "js", "jsx", "ts", "tsx", "py"];
        
        for entry in WalkDir::new(project_path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            
            // Skip non-source files
            if let Some(ext) = path.extension() {
                if !supported_extensions.contains(&ext.to_str().unwrap_or("")) {
                    continue;
                }
            } else {
                continue;
            }
            
            // Skip common ignored directories
            if path.components().any(|c| {
                matches!(c.as_os_str().to_str(), Some("node_modules") | Some("target") | Some(".git") | Some("dist") | Some("build"))
            }) {
                continue;
            }
            
            match self.analyze_file(path).await {
                Ok(file_entities) => {
                    debug!("Analyzed {}: found {} entities", path.display(), file_entities.len());
                    entities.extend(file_entities);
                }
                Err(e) => {
                    warn!("Failed to analyze {}: {}", path.display(), e);
                }
            }
        }
        
        info!("Project analysis complete: found {} entities", entities.len());
        Ok(entities)
    }
    
    pub async fn analyze_file(&self, file_path: &Path) -> Result<Vec<CodeEntity>> {
        let content = std::fs::read_to_string(file_path)
            .map_err(|e| anyhow!("Failed to read file: {}", e))?;
        
        let language = self.detect_language(file_path)?;
        let mut entities = Vec::new();
        
        // Create file entity
        let file_entity = CodeEntity {
            id: Uuid::new_v4().to_string(),
            entity_type: EntityType::File,
            name: file_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string(),
            file_path: file_path.to_string_lossy().to_string(),
            line_start: 1,
            line_end: content.lines().count() as u32,
            content: content.clone(),
            language: language.clone(),
            metadata: serde_json::json!({
                "size": content.len(),
                "hash": self.hash_content(&content),
            }),
        };
        entities.push(file_entity.clone());
        
        // Parse file content
        let mut parsers = self.parsers.write().await;
        let parser = match language.as_str() {
            "rust" => &mut parsers.rust,
            "javascript" | "typescript" => &mut parsers.javascript,
            "python" => &mut parsers.python,
            _ => return Ok(entities), // Return just file entity for unsupported languages
        };
        
        if let Some(tree) = parser.parse(&content, None) {
            let root_node = tree.root_node();
            let mut cursor = root_node.walk();
            
            // Extract entities based on language
            match language.as_str() {
                "rust" => self.extract_rust_entities(&file_entity, root_node, &content, &mut entities)?,
                "javascript" | "typescript" => self.extract_javascript_entities(&file_entity, root_node, &content, &mut entities)?,
                "python" => self.extract_python_entities(&file_entity, root_node, &content, &mut entities)?,
                _ => {}
            }
        }
        
        Ok(entities)
    }
    
    fn extract_rust_entities(
        &self,
        file_entity: &CodeEntity,
        node: Node,
        source: &str,
        entities: &mut Vec<CodeEntity>,
    ) -> Result<()> {
        let mut cursor = node.walk();
        
        for child in node.children(&mut cursor) {
            match child.kind() {
                "function_item" | "method_item" => {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        let entity = CodeEntity {
                            id: Uuid::new_v4().to_string(),
                            entity_type: if child.kind() == "method_item" { 
                                EntityType::Method 
                            } else { 
                                EntityType::Function 
                            },
                            name: name_node.utf8_text(source.as_bytes())?.to_string(),
                            file_path: file_entity.file_path.clone(),
                            line_start: child.start_position().row as u32 + 1,
                            line_end: child.end_position().row as u32 + 1,
                            content: child.utf8_text(source.as_bytes())?.to_string(),
                            language: "rust".to_string(),
                            metadata: serde_json::json!({}),
                        };
                        entities.push(entity);
                    }
                }
                "struct_item" | "impl_item" | "trait_item" | "enum_item" => {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        let entity = CodeEntity {
                            id: Uuid::new_v4().to_string(),
                            entity_type: EntityType::Class,
                            name: name_node.utf8_text(source.as_bytes())?.to_string(),
                            file_path: file_entity.file_path.clone(),
                            line_start: child.start_position().row as u32 + 1,
                            line_end: child.end_position().row as u32 + 1,
                            content: child.utf8_text(source.as_bytes())?.to_string(),
                            language: "rust".to_string(),
                            metadata: serde_json::json!({
                                "kind": child.kind()
                            }),
                        };
                        entities.push(entity);
                    }
                }
                "use_declaration" => {
                    let entity = CodeEntity {
                        id: Uuid::new_v4().to_string(),
                        entity_type: EntityType::Import,
                        name: child.utf8_text(source.as_bytes())?.to_string(),
                        file_path: file_entity.file_path.clone(),
                        line_start: child.start_position().row as u32 + 1,
                        line_end: child.end_position().row as u32 + 1,
                        content: child.utf8_text(source.as_bytes())?.to_string(),
                        language: "rust".to_string(),
                        metadata: serde_json::json!({}),
                    };
                    entities.push(entity);
                }
                _ => {
                    // Recursively process child nodes
                    self.extract_rust_entities(file_entity, child, source, entities)?;
                }
            }
        }
        
        Ok(())
    }
    
    fn extract_javascript_entities(
        &self,
        file_entity: &CodeEntity,
        node: Node,
        source: &str,
        entities: &mut Vec<CodeEntity>,
    ) -> Result<()> {
        let mut cursor = node.walk();
        
        for child in node.children(&mut cursor) {
            match child.kind() {
                "function_declaration" | "arrow_function" | "function_expression" => {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        let entity = CodeEntity {
                            id: Uuid::new_v4().to_string(),
                            entity_type: EntityType::Function,
                            name: name_node.utf8_text(source.as_bytes())?.to_string(),
                            file_path: file_entity.file_path.clone(),
                            line_start: child.start_position().row as u32 + 1,
                            line_end: child.end_position().row as u32 + 1,
                            content: child.utf8_text(source.as_bytes())?.to_string(),
                            language: "javascript".to_string(),
                            metadata: serde_json::json!({}),
                        };
                        entities.push(entity);
                    }
                }
                "class_declaration" => {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        let entity = CodeEntity {
                            id: Uuid::new_v4().to_string(),
                            entity_type: EntityType::Class,
                            name: name_node.utf8_text(source.as_bytes())?.to_string(),
                            file_path: file_entity.file_path.clone(),
                            line_start: child.start_position().row as u32 + 1,
                            line_end: child.end_position().row as u32 + 1,
                            content: child.utf8_text(source.as_bytes())?.to_string(),
                            language: "javascript".to_string(),
                            metadata: serde_json::json!({}),
                        };
                        entities.push(entity);
                    }
                }
                "import_statement" => {
                    let entity = CodeEntity {
                        id: Uuid::new_v4().to_string(),
                        entity_type: EntityType::Import,
                        name: child.utf8_text(source.as_bytes())?.to_string(),
                        file_path: file_entity.file_path.clone(),
                        line_start: child.start_position().row as u32 + 1,
                        line_end: child.end_position().row as u32 + 1,
                        content: child.utf8_text(source.as_bytes())?.to_string(),
                        language: "javascript".to_string(),
                        metadata: serde_json::json!({}),
                    };
                    entities.push(entity);
                }
                _ => {
                    // Recursively process child nodes
                    self.extract_javascript_entities(file_entity, child, source, entities)?;
                }
            }
        }
        
        Ok(())
    }
    
    fn extract_python_entities(
        &self,
        file_entity: &CodeEntity,
        node: Node,
        source: &str,
        entities: &mut Vec<CodeEntity>,
    ) -> Result<()> {
        let mut cursor = node.walk();
        
        for child in node.children(&mut cursor) {
            match child.kind() {
                "function_definition" => {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        let entity = CodeEntity {
                            id: Uuid::new_v4().to_string(),
                            entity_type: EntityType::Function,
                            name: name_node.utf8_text(source.as_bytes())?.to_string(),
                            file_path: file_entity.file_path.clone(),
                            line_start: child.start_position().row as u32 + 1,
                            line_end: child.end_position().row as u32 + 1,
                            content: child.utf8_text(source.as_bytes())?.to_string(),
                            language: "python".to_string(),
                            metadata: serde_json::json!({}),
                        };
                        entities.push(entity);
                    }
                }
                "class_definition" => {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        let entity = CodeEntity {
                            id: Uuid::new_v4().to_string(),
                            entity_type: EntityType::Class,
                            name: name_node.utf8_text(source.as_bytes())?.to_string(),
                            file_path: file_entity.file_path.clone(),
                            line_start: child.start_position().row as u32 + 1,
                            line_end: child.end_position().row as u32 + 1,
                            content: child.utf8_text(source.as_bytes())?.to_string(),
                            language: "python".to_string(),
                            metadata: serde_json::json!({}),
                        };
                        entities.push(entity);
                    }
                }
                "import_statement" | "import_from_statement" => {
                    let entity = CodeEntity {
                        id: Uuid::new_v4().to_string(),
                        entity_type: EntityType::Import,
                        name: child.utf8_text(source.as_bytes())?.to_string(),
                        file_path: file_entity.file_path.clone(),
                        line_start: child.start_position().row as u32 + 1,
                        line_end: child.end_position().row as u32 + 1,
                        content: child.utf8_text(source.as_bytes())?.to_string(),
                        language: "python".to_string(),
                        metadata: serde_json::json!({}),
                    };
                    entities.push(entity);
                }
                _ => {
                    // Recursively process child nodes
                    self.extract_python_entities(file_entity, child, source, entities)?;
                }
            }
        }
        
        Ok(())
    }
    
    fn detect_language(&self, file_path: &Path) -> Result<String> {
        let ext = file_path.extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| anyhow!("No file extension"))?;
        
        match ext {
            "rs" => Ok("rust".to_string()),
            "js" | "jsx" => Ok("javascript".to_string()),
            "ts" | "tsx" => Ok("typescript".to_string()),
            "py" => Ok("python".to_string()),
            _ => Err(anyhow!("Unsupported language: {}", ext))
        }
    }
    
    fn hash_content(&self, content: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
    
    pub fn start_file_watcher(&mut self, path: PathBuf, tx: mpsc::Sender<PathBuf>) -> Result<()> {
        let (notify_tx, notify_rx) = mpsc::channel();
        
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        for path in event.paths {
                            let _ = notify_tx.send(path);
                        }
                    }
                    _ => {}
                }
            }
        })?;
        
        watcher.watch(&path, RecursiveMode::Recursive)?;
        
        // Forward events to the provided channel
        std::thread::spawn(move || {
            while let Ok(path) = notify_rx.recv() {
                let _ = tx.send(path);
            }
        });
        
        self.file_watcher = Some(watcher);
        info!("File watcher started for: {:?}", path);
        
        Ok(())
    }
}