use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheNamespace {
    Entity,
    Search,
    Sparql,
}

impl CacheNamespace {
    fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Search => "search",
            Self::Sparql => "sparql",
        }
    }
}

pub fn entity_key(id: &str, params: &[(&str, &str)]) -> String {
    rest_key(CacheNamespace::Entity, id, params)
}

pub fn search_key(query: &str, params: &[(&str, &str)]) -> String {
    rest_key(CacheNamespace::Search, query, params)
}

pub fn sparql_key(query: &str) -> String {
    sparql_key_with_params(query, &[])
}

pub fn sparql_key_with_params(query: &str, params: &[(&str, &str)]) -> String {
    let normalized_query = normalize_sparql_whitespace(query);
    rest_key(CacheNamespace::Sparql, normalized_query.as_str(), params)
}

fn rest_key(namespace: CacheNamespace, resource: &str, params: &[(&str, &str)]) -> String {
    let mut normalized_params: Vec<_> = params.iter().copied().collect();
    normalized_params.sort_unstable_by(|left, right| left.0.cmp(right.0).then(left.1.cmp(right.1)));

    let params = normalized_params
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");

    digest_parts([namespace.as_str(), resource, params.as_str()])
}

fn normalize_sparql_whitespace(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn digest_parts<const N: usize>(parts: [&str; N]) -> String {
    let mut hasher = Sha256::new();

    for part in parts {
        hasher.update(part.len().to_be_bytes());
        hasher.update(part.as_bytes());
    }

    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_params_are_order_independent() {
        let first = entity_key("Albert_Einstein", &[("format", "json"), ("lang", "en")]);
        let second = entity_key("Albert_Einstein", &[("lang", "en"), ("format", "json")]);

        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn different_rest_resources_and_namespaces_get_different_keys() {
        let entity = entity_key("Albert_Einstein", &[("lang", "en")]);
        let other_entity = entity_key("Marie_Curie", &[("lang", "en")]);
        let search = search_key("Albert Einstein", &[("lang", "en")]);

        assert_ne!(entity, other_entity);
        assert_ne!(entity, search);
    }

    #[test]
    fn raw_sparql_key_uses_normalized_whitespace() {
        let first = sparql_key("SELECT * WHERE { ?s ?p ?o } LIMIT 1");
        let second = sparql_key("SELECT   *\nWHERE {\n?s ?p ?o\n}\nLIMIT 1");
        let third = sparql_key("SELECT * WHERE { ?s ?p ?o } LIMIT 2");

        assert_eq!(first, second);
        assert_ne!(first, third);
    }
}
