use std::collections::HashMap;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// A query template with name, description, and Cypher query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryTemplate {
    pub name: String,
    pub description: String,
    pub cypher: String,
    pub parameters: Vec<String>,
}

lazy_static! {
    /// Common query templates for quick access
    static ref QUERY_TEMPLATES: HashMap<String, QueryTemplate> = {
        let mut templates = HashMap::new();
        
        // Unused code detection
        templates.insert("unused_functions".to_string(), QueryTemplate {
            name: "Unused Functions".to_string(),
            description: "Find functions that are never called".to_string(),
            cypher: "MATCH (f:Function) WHERE NOT (()-[:CALLS]->(f)) AND f.name <> 'main' RETURN f.name as name, f.file_path as file, f.line_start as line ORDER BY f.file_path, f.line_start".to_string(),
            parameters: vec![],
        });
        
        // Circular dependencies
        templates.insert("circular_dependencies".to_string(), QueryTemplate {
            name: "Circular Dependencies".to_string(),
            description: "Find circular dependencies between modules".to_string(),
            cypher: "MATCH (a:Module)-[:IMPORTS]->(b:Module)-[:IMPORTS]->(a) RETURN DISTINCT a.name as module1, b.name as module2".to_string(),
            parameters: vec![],
        });
        
        // Most connected entities
        templates.insert("most_connected".to_string(), QueryTemplate {
            name: "Most Connected Entities".to_string(),
            description: "Find the most connected code entities".to_string(),
            cypher: "MATCH (e:Entity)-[r]-() WITH e, count(r) as connections WHERE connections > 5 RETURN e.name as name, e.entity_type as type, connections ORDER BY connections DESC LIMIT 20".to_string(),
            parameters: vec![],
        });
        
        // Recent changes
        templates.insert("recent_changes".to_string(), QueryTemplate {
            name: "Recent Changes".to_string(),
            description: "Find recently modified entities".to_string(),
            cypher: "MATCH (e:Entity) WHERE e.updated_at > datetime() - duration('P1D') RETURN e.name as name, e.entity_type as type, e.file_path as file, e.updated_at as updated ORDER BY e.updated_at DESC".to_string(),
            parameters: vec![],
        });
        
        // TODO/FIXME comments
        templates.insert("todos".to_string(), QueryTemplate {
            name: "TODO Comments".to_string(),
            description: "Find all TODO and FIXME comments".to_string(),
            cypher: "MATCH (e:Entity) WHERE e.content =~ '.*\\\\b(TODO|FIXME|XXX|HACK|BUG)\\\\b.*' RETURN e.file_path as file, e.line_start as line, trim(split(e.content, '\\n')[0]) as comment ORDER BY e.file_path, e.line_start".to_string(),
            parameters: vec![],
        });
        
        // Large functions
        templates.insert("large_functions".to_string(), QueryTemplate {
            name: "Large Functions".to_string(),
            description: "Find functions with many lines of code".to_string(),
            cypher: "MATCH (f:Function) WHERE (f.line_end - f.line_start) > 50 RETURN f.name as name, f.file_path as file, (f.line_end - f.line_start) as lines ORDER BY lines DESC".to_string(),
            parameters: vec![],
        });
        
        // Orphaned files
        templates.insert("orphaned_files".to_string(), QueryTemplate {
            name: "Orphaned Files".to_string(),
            description: "Find files that are never imported".to_string(),
            cypher: "MATCH (f:File) WHERE NOT (()-[:IMPORTS]->(:Entity)<-[:CONTAINS]-(f)) AND f.path <> 'main' RETURN f.path as file ORDER BY f.path".to_string(),
            parameters: vec![],
        });
        
        // Complex functions (many dependencies)
        templates.insert("complex_functions".to_string(), QueryTemplate {
            name: "Complex Functions".to_string(),
            description: "Find functions with many dependencies".to_string(),
            cypher: "MATCH (f:Function)-[r:CALLS|USES|IMPORTS]->() WITH f, count(r) as dependencies WHERE dependencies > 10 RETURN f.name as name, f.file_path as file, dependencies ORDER BY dependencies DESC".to_string(),
            parameters: vec![],
        });
        
        // Test coverage gaps
        templates.insert("untested_functions".to_string(), QueryTemplate {
            name: "Untested Functions".to_string(),
            description: "Find functions without associated tests".to_string(),
            cypher: "MATCH (f:Function) WHERE NOT f.file_path CONTAINS 'test' AND NOT EXISTS { MATCH (t:Function) WHERE t.file_path CONTAINS 'test' AND (t.name CONTAINS f.name OR t.content CONTAINS f.name) } RETURN f.name as name, f.file_path as file ORDER BY f.file_path".to_string(),
            parameters: vec![],
        });
        
        // Duplicate function names
        templates.insert("duplicate_names".to_string(), QueryTemplate {
            name: "Duplicate Function Names".to_string(),
            description: "Find functions with the same name in different files".to_string(),
            cypher: "MATCH (f1:Function), (f2:Function) WHERE f1.name = f2.name AND f1.id < f2.id AND f1.file_path <> f2.file_path RETURN f1.name as name, collect(DISTINCT f1.file_path) + collect(DISTINCT f2.file_path) as files ORDER BY name".to_string(),
            parameters: vec![],
        });
        
        templates
    };
    
    /// Regex patterns for template-based queries
    static ref TEMPLATE_PATTERNS: Vec<(Regex, &'static str)> = vec![
        (Regex::new(r"(?i)unused\s+functions?").unwrap(), "unused_functions"),
        (Regex::new(r"(?i)circular\s+dep").unwrap(), "circular_dependencies"),
        (Regex::new(r"(?i)most\s+connected").unwrap(), "most_connected"),
        (Regex::new(r"(?i)recent\s+changes?").unwrap(), "recent_changes"),
        (Regex::new(r"(?i)(todo|fixme)s?").unwrap(), "todos"),
        (Regex::new(r"(?i)large\s+functions?").unwrap(), "large_functions"),
        (Regex::new(r"(?i)orphaned?\s+files?").unwrap(), "orphaned_files"),
        (Regex::new(r"(?i)complex\s+functions?").unwrap(), "complex_functions"),
        (Regex::new(r"(?i)untested\s+functions?").unwrap(), "untested_functions"),
        (Regex::new(r"(?i)duplicate\s+(function\s+)?names?").unwrap(), "duplicate_names"),
    ];
}

/// Get a query template by name
pub fn get_query_template(name: &str) -> Option<&'static QueryTemplate> {
    QUERY_TEMPLATES.get(name)
}

/// Get all available query templates
pub fn list_query_templates() -> Vec<&'static QueryTemplate> {
    QUERY_TEMPLATES.values().collect()
}

/// Match a natural language query to a template
pub fn match_query_to_template(query: &str) -> Option<&'static QueryTemplate> {
    for (pattern, template_name) in TEMPLATE_PATTERNS.iter() {
        if pattern.is_match(query) {
            return QUERY_TEMPLATES.get(*template_name);
        }
    }
    None
}

/// Build a parameterized query from a template
pub fn build_query_from_template(template_name: &str, params: &HashMap<String, String>) -> Option<String> {
    QUERY_TEMPLATES.get(template_name).map(|template| {
        let mut query = template.cypher.clone();
        
        // Replace parameters in the query
        for (key, value) in params {
            let placeholder = format!("${{{}}}", key);
            query = query.replace(&placeholder, value);
        }
        
        query
    })
}

/// Get suggested queries based on partial input
pub fn suggest_queries(partial: &str) -> Vec<&'static str> {
    let partial_lower = partial.to_lowercase();
    QUERY_TEMPLATES
        .iter()
        .filter(|(_, template)| {
            template.name.to_lowercase().contains(&partial_lower) ||
            template.description.to_lowercase().contains(&partial_lower)
        })
        .map(|(key, _)| key.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_get_query_template() {
        let template = get_query_template("unused_functions");
        assert!(template.is_some());
        assert_eq!(template.unwrap().name, "Unused Functions");
    }
    
    #[test]
    fn test_match_query_to_template() {
        assert!(match_query_to_template("show me unused functions").is_some());
        assert!(match_query_to_template("find circular dependencies").is_some());
        assert!(match_query_to_template("what are the TODOs").is_some());
        assert!(match_query_to_template("random query").is_none());
    }
    
    #[test]
    fn test_suggest_queries() {
        let suggestions = suggest_queries("unused");
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"unused_functions"));
        
        let suggestions = suggest_queries("function");
        assert!(suggestions.len() > 1);
    }
}