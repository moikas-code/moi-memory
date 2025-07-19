use regex::Regex;
use lazy_static::lazy_static;
use std::collections::HashMap;

/// Enhanced natural language pattern matching for more flexible query understanding
pub struct NLPPatternMatcher {
    patterns: Vec<QueryPattern>,
}

struct QueryPattern {
    name: String,
    patterns: Vec<Regex>,
    cypher_template: String,
    extract_params: fn(&str, &Regex) -> Option<HashMap<String, String>>,
}

lazy_static! {
    // Common variations for "show/find/get/list"
    static ref SHOW_VERBS: String = r"(?:show|find|get|list|display|fetch|retrieve|search for|look for|give me|what are|which are)".to_string();
    
    // Common variations for code entities
    static ref ENTITY_TYPES: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert("functions", r"(?:functions?|methods?|procedures?|routines?)");
        m.insert("classes", r"(?:classes|types?|structs?|interfaces?)");
        m.insert("files", r"(?:files?|modules?|scripts?|sources?)");
        m.insert("variables", r"(?:variables?|constants?|fields?|properties?)");
        m
    };
}

impl NLPPatternMatcher {
    pub fn new() -> Self {
        let patterns = vec![
            // Pattern: "show me all functions in file X"
            QueryPattern {
                name: "functions_in_file".to_string(),
                patterns: vec![
                    Regex::new(&format!(r"{} all? ?{} in (?:file |the )?(.+)", SHOW_VERBS.as_str(), ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(&format!(r"what {} are in (?:file |the )?(.+)", ENTITY_TYPES["functions"])).unwrap(),
                ],
                cypher_template: "MATCH (f:File {path: $file})-[:CONTAINS]->(fn:Function) RETURN fn.name as name, fn.line_start as line".to_string(),
                extract_params: |text, regex| {
                    regex.captures(text).map(|caps| {
                        let mut params = HashMap::new();
                        params.insert("file".to_string(), caps.get(1)?.as_str().to_string());
                        params
                    })
                },
            },
            
            // Pattern: "functions that call X"
            QueryPattern {
                name: "callers_of_function".to_string(),
                patterns: vec![
                    Regex::new(&format!(r"{} (?:that |which )?calls? (.+)", ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(r"who calls (.+)").unwrap(),
                    Regex::new(r"callers of (.+)").unwrap(),
                ],
                cypher_template: "MATCH (caller:Function)-[:CALLS]->(callee:Function {name: $function}) RETURN caller.name as caller, caller.file_path as file".to_string(),
                extract_params: |text, regex| {
                    regex.captures(text).map(|caps| {
                        let mut params = HashMap::new();
                        params.insert("function".to_string(), caps.get(1)?.as_str().trim_matches('"').to_string());
                        params
                    })
                },
            },
            
            // Pattern: "functions called by X"
            QueryPattern {
                name: "callees_of_function".to_string(),
                patterns: vec![
                    Regex::new(&format!(r"{} called by (.+)", ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(r"what does (.+) call").unwrap(),
                    Regex::new(r"callees of (.+)").unwrap(),
                ],
                cypher_template: "MATCH (caller:Function {name: $function})-[:CALLS]->(callee:Function) RETURN callee.name as callee, callee.file_path as file".to_string(),
                extract_params: |text, regex| {
                    regex.captures(text).map(|caps| {
                        let mut params = HashMap::new();
                        params.insert("function".to_string(), caps.get(1)?.as_str().trim_matches('"').to_string());
                        params
                    })
                },
            },
            
            // Pattern: "classes that inherit from X"
            QueryPattern {
                name: "subclasses".to_string(),
                patterns: vec![
                    Regex::new(&format!(r"{} (?:that |which )?(?:inherit|extend|derive) from (.+)", ENTITY_TYPES["classes"])).unwrap(),
                    Regex::new(r"subclasses of (.+)").unwrap(),
                    Regex::new(r"children of (.+)").unwrap(),
                ],
                cypher_template: "MATCH (child:Class)-[:INHERITS]->(parent:Class {name: $class}) RETURN child.name as subclass, child.file_path as file".to_string(),
                extract_params: |text, regex| {
                    regex.captures(text).map(|caps| {
                        let mut params = HashMap::new();
                        params.insert("class".to_string(), caps.get(1)?.as_str().trim_matches('"').to_string());
                        params
                    })
                },
            },
            
            // Pattern: "dependencies between X and Y"
            QueryPattern {
                name: "dependencies_between".to_string(),
                patterns: vec![
                    Regex::new(r"dependencies between (.+) and (.+)").unwrap(),
                    Regex::new(r"how (?:does |is )(.+) (?:depend on|related to|connected to) (.+)").unwrap(),
                ],
                cypher_template: "MATCH path=shortestPath((a:Entity {name: $entity1})-[*]-(b:Entity {name: $entity2})) RETURN path".to_string(),
                extract_params: |text, regex| {
                    regex.captures(text).map(|caps| {
                        let mut params = HashMap::new();
                        params.insert("entity1".to_string(), caps.get(1)?.as_str().trim_matches('"').to_string());
                        params.insert("entity2".to_string(), caps.get(2)?.as_str().trim_matches('"').to_string());
                        params
                    })
                },
            },
            
            // Pattern: "most complex functions"
            QueryPattern {
                name: "complex_functions".to_string(),
                patterns: vec![
                    Regex::new(r"most complex {}".format(ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(r"complicated {}".format(ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(r"{} with (?:high|most) complexity".format(ENTITY_TYPES["functions"])).unwrap(),
                ],
                cypher_template: "MATCH (f:Function) WHERE size(split(f.content, '\\n')) > 50 RETURN f.name as name, f.file_path as file, size(split(f.content, '\\n')) as lines ORDER BY lines DESC LIMIT 10".to_string(),
                extract_params: |_, _| Some(HashMap::new()),
            },
            
            // Pattern: "recently modified"
            QueryPattern {
                name: "recent_changes".to_string(),
                patterns: vec![
                    Regex::new(r"recently (?:modified|changed|updated)").unwrap(),
                    Regex::new(r"latest (?:changes|modifications|updates)").unwrap(),
                    Regex::new(r"what (?:changed|was modified) recently").unwrap(),
                ],
                cypher_template: "MATCH (e:Entity) WHERE exists(e.updated_at) RETURN e.name as name, e.entity_type as type, e.file_path as file, e.updated_at as updated ORDER BY e.updated_at DESC LIMIT 20".to_string(),
                extract_params: |_, _| Some(HashMap::new()),
            },
            
            // Pattern: "functions with more than X lines"
            QueryPattern {
                name: "large_functions_threshold".to_string(),
                patterns: vec![
                    Regex::new(r"{} with more than (\d+) lines".format(ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(r"{} (?:larger|bigger) than (\d+) lines".format(ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(r"(?:large|big) {} over (\d+) lines".format(ENTITY_TYPES["functions"])).unwrap(),
                ],
                cypher_template: "MATCH (f:Function) WHERE (f.line_end - f.line_start) > $threshold RETURN f.name as name, f.file_path as file, (f.line_end - f.line_start) as lines ORDER BY lines DESC".to_string(),
                extract_params: |text, regex| {
                    regex.captures(text).map(|caps| {
                        let mut params = HashMap::new();
                        params.insert("threshold".to_string(), caps.get(1)?.as_str().to_string());
                        params
                    })
                },
            },
            
            // Pattern: "imports in file X"
            QueryPattern {
                name: "imports_in_file".to_string(),
                patterns: vec![
                    Regex::new(r"imports in (?:file |the )?(.+)").unwrap(),
                    Regex::new(r"what does (.+) import").unwrap(),
                    Regex::new(r"dependencies of (?:file |the )?(.+)").unwrap(),
                ],
                cypher_template: "MATCH (f:File {path: $file})-[:CONTAINS]->(i:Import) RETURN i.name as import, i.line_start as line".to_string(),
                extract_params: |text, regex| {
                    regex.captures(text).map(|caps| {
                        let mut params = HashMap::new();
                        params.insert("file".to_string(), caps.get(1)?.as_str().to_string());
                        params
                    })
                },
            },
            
            // Pattern: "public API" or "exported functions"
            QueryPattern {
                name: "public_api".to_string(),
                patterns: vec![
                    Regex::new(r"public (?:API|interface|{})"r.format(ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(r"exported {}".format(ENTITY_TYPES["functions"])).unwrap(),
                    Regex::new(r"external (?:API|interface)").unwrap(),
                ],
                cypher_template: "MATCH (f:Function) WHERE f.metadata CONTAINS '\"visibility\":\"public\"' OR f.metadata CONTAINS '\"exported\":true' RETURN f.name as name, f.file_path as file".to_string(),
                extract_params: |_, _| Some(HashMap::new()),
            },
        ];
        
        Self { patterns }
    }
    
    pub fn match_query(&self, query: &str) -> Option<(String, HashMap<String, String>)> {
        let query_lower = query.to_lowercase();
        
        for pattern in &self.patterns {
            for regex in &pattern.patterns {
                if regex.is_match(&query_lower) {
                    if let Some(params) = (pattern.extract_params)(&query_lower, regex) {
                        // Replace parameters in the template
                        let mut cypher = pattern.cypher_template.clone();
                        for (key, value) in &params {
                            cypher = cypher.replace(&format!("${}", key), value);
                        }
                        return Some((cypher, params));
                    }
                }
            }
        }
        
        None
    }
    
    pub fn get_supported_patterns(&self) -> Vec<String> {
        self.patterns.iter()
            .flat_map(|p| p.patterns.iter().map(|r| r.to_string()))
            .collect()
    }
}

/// Enhanced query understanding with fuzzy matching and context awareness
pub fn enhance_natural_language_query(query: &str) -> Option<String> {
    let matcher = NLPPatternMatcher::new();
    
    if let Some((cypher, _)) = matcher.match_query(query) {
        return Some(cypher);
    }
    
    // Additional fuzzy matching for common typos and variations
    let query_lower = query.to_lowercase();
    let corrections = vec![
        ("fucntion", "function"),
        ("calss", "class"),
        ("fiel", "file"),
        ("improt", "import"),
        ("inhertis", "inherits"),
    ];
    
    let mut corrected_query = query_lower.clone();
    for (typo, correct) in corrections {
        corrected_query = corrected_query.replace(typo, correct);
    }
    
    if corrected_query != query_lower {
        if let Some((cypher, _)) = matcher.match_query(&corrected_query) {
            return Some(cypher);
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_function_patterns() {
        let matcher = NLPPatternMatcher::new();
        
        let queries = vec![
            "show me all functions in main.rs",
            "what functions are in main.rs",
            "list functions in main.rs",
        ];
        
        for query in queries {
            let result = matcher.match_query(query);
            assert!(result.is_some(), "Failed to match: {}", query);
            let (cypher, params) = result.unwrap();
            assert!(cypher.contains("MATCH (f:File {path: main.rs})"));
            assert_eq!(params.get("file"), Some(&"main.rs".to_string()));
        }
    }
    
    #[test]
    fn test_caller_patterns() {
        let matcher = NLPPatternMatcher::new();
        
        let queries = vec![
            "functions that call process_data",
            "who calls process_data",
            "callers of process_data",
        ];
        
        for query in queries {
            let result = matcher.match_query(query);
            assert!(result.is_some(), "Failed to match: {}", query);
            let (cypher, _) = result.unwrap();
            assert!(cypher.contains("CALLS"));
            assert!(cypher.contains("process_data"));
        }
    }
    
    #[test]
    fn test_fuzzy_matching() {
        let result = enhance_natural_language_query("show me all fucntions in main.rs");
        assert!(result.is_some());
        assert!(result.unwrap().contains("Function"));
    }
}