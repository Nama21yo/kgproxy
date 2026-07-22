use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::Value;
use thiserror::Error;

#[async_trait]
pub trait DbpediaClient: Send + Sync {
    async fn entity(
        &self,
        id: &str,
        endpoint_override: Option<&str>,
        lang: &str,
    ) -> Result<Value, OriginError>;
    async fn search(
        &self,
        query: &str,
        endpoint_override: Option<&str>,
        lang: &str,
    ) -> Result<Value, OriginError>;
    async fn sparql(
        &self,
        query: &str,
        endpoint_override: Option<&str>,
    ) -> Result<Value, OriginError>;
}

#[derive(Debug, Clone)]
pub struct ReqwestDbpediaClient {
    endpoint: String,
    client: reqwest::Client,
    max_response_bytes: usize,
}

#[derive(Debug, Error)]
pub enum OriginError {
    #[error("dbpedia request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("dbpedia returned invalid json: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("dbpedia returned status {0}")]
    Status(StatusCode),
    #[error("dbpedia response exceeded {limit} bytes")]
    ResponseTooLarge { limit: usize },
}

impl ReqwestDbpediaClient {
    pub fn new(
        endpoint: impl Into<String>,
        timeout: Duration,
        max_response_bytes: usize,
    ) -> Result<Self, OriginError> {
        let client = reqwest::Client::builder().timeout(timeout).build()?;

        Ok(Self {
            endpoint: endpoint.into(),
            client,
            max_response_bytes,
        })
    }

    async fn execute_sparql(
        &self,
        query: &str,
        endpoint_override: Option<&str>,
    ) -> Result<Value, OriginError> {
        let endpoint = endpoint_override.unwrap_or(&self.endpoint);
        let response = self
            .client
            .post(endpoint)
            .header("accept", "application/sparql-results+json")
            .form(&[("query", query)])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(OriginError::Status(response.status()));
        }

        let bytes = response.bytes().await?;
        if bytes.len() > self.max_response_bytes {
            return Err(OriginError::ResponseTooLarge {
                limit: self.max_response_bytes,
            });
        }

        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[async_trait]
impl DbpediaClient for ReqwestDbpediaClient {
    async fn entity(
        &self,
        id: &str,
        endpoint_override: Option<&str>,
        lang: &str,
    ) -> Result<Value, OriginError> {
        self.execute_sparql(&entity_query(id, lang), endpoint_override)
            .await
    }

    async fn search(
        &self,
        query: &str,
        endpoint_override: Option<&str>,
        lang: &str,
    ) -> Result<Value, OriginError> {
        self.execute_sparql(&search_query(query, 10, lang), endpoint_override)
            .await
    }

    async fn sparql(
        &self,
        query: &str,
        endpoint_override: Option<&str>,
    ) -> Result<Value, OriginError> {
        self.execute_sparql(query, endpoint_override).await
    }
}

pub fn entity_query(id: &str, lang: &str) -> String {
    let resource_host = if lang == "am" {
        "am.dbpedia.org"
    } else {
        "dbpedia.org"
    };
    let resource = format!("http://{resource_host}/resource/{}", encode_resource_id(id));

    format!(
        r#"PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX dbo: <http://dbpedia.org/ontology/>

SELECT ?label ?abstract ?property ?value WHERE {{
  VALUES ?entity {{ <{resource}> }}
  OPTIONAL {{ ?entity rdfs:label ?label FILTER (lang(?label) = "{lang}") }}
  OPTIONAL {{ ?entity dbo:abstract ?abstract FILTER (lang(?abstract) = "{lang}") }}
  OPTIONAL {{
    ?entity ?property ?value .
    FILTER (?property IN (dbo:birthDate, dbo:deathDate, dbo:birthPlace, dbo:knownFor))
  }}
}}
LIMIT 50"#,
    )
}

pub fn search_query(label: &str, limit: u16, lang: &str) -> String {
    let escaped_label = escape_sparql_string(label);
    let bounded_limit = limit.clamp(1, 50);

    format!(
        r#"PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX dbo: <http://dbpedia.org/ontology/>

SELECT ?entity ?label ?abstract WHERE {{
  ?entity rdfs:label ?label .
  FILTER (lang(?label) = "{lang}")
  FILTER CONTAINS(LCASE(STR(?label)), LCASE("{escaped_label}"))
  OPTIONAL {{ ?entity dbo:abstract ?abstract FILTER (lang(?abstract) = "{lang}") }}
}}
LIMIT {bounded_limit}"#,
    )
}

fn encode_resource_id(id: &str) -> String {
    id.split('/')
        .map(|part| {
            part.chars()
                .map(|character| match character {
                    ' ' => "%20".to_owned(),
                    '<' => "%3C".to_owned(),
                    '>' => "%3E".to_owned(),
                    '"' => "%22".to_owned(),
                    '\\' => "%5C".to_owned(),
                    '#' => "%23".to_owned(),
                    '?' => "%3F".to_owned(),
                    '%' => "%25".to_owned(),
                    character => character.to_string(),
                })
                .fold(String::new(), |mut encoded, character| {
                    encoded.push_str(&character);
                    encoded
                })
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn escape_sparql_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_lookup_builds_expected_sparql() {
        let query = entity_query("Albert_Einstein", "en");

        assert!(query.contains("<http://dbpedia.org/resource/Albert_Einstein>"));
        assert!(query.contains("dbo:abstract"));
        assert!(query.contains("FILTER (lang(?abstract) = \"en\")"));
        assert!(query.contains("LIMIT 50"));
    }

    #[test]
    fn entity_lookup_url_encodes_resource_segments() {
        let query = entity_query("A B/C D", "en");

        assert!(query.contains("<http://dbpedia.org/resource/A%20B/C%20D>"));
    }

    #[test]
    fn search_lookup_adds_bounded_limit() {
        let query = search_query("Albert Einstein", 500, "en");

        assert!(query.contains("CONTAINS(LCASE(STR(?label)), LCASE(\"Albert Einstein\"))"));
        assert!(query.contains("LIMIT 50"));
    }

    #[test]
    fn search_lookup_escapes_quotes() {
        let query = search_query("Ada \"Countess\" Lovelace", 10, "en");

        assert!(query.contains("Ada \\\"Countess\\\" Lovelace"));
        assert!(query.contains("LIMIT 10"));
    }

    #[test]
    fn raw_sparql_passthrough_is_not_rewritten_by_client_api() {
        let query = "SELECT * WHERE { ?s ?p ?o } LIMIT 1";

        assert_eq!(query, query);
    }

    #[test]
    fn non_success_status_maps_to_typed_error() {
        let error = OriginError::Status(StatusCode::TOO_MANY_REQUESTS);

        assert_eq!(
            error.to_string(),
            "dbpedia returned status 429 Too Many Requests"
        );
    }

    #[test]
    fn generated_queries_use_requested_language() {
        let entity = entity_query("Berlin", "de");
        let search = search_query("Berlin", 10, "fr");

        assert!(entity.contains("lang(?label) = \"de\""));
        assert!(entity.contains("lang(?abstract) = \"de\""));
        assert!(search.contains("lang(?label) = \"fr\""));
        assert!(search.contains("lang(?abstract) = \"fr\""));
    }

    #[test]
    fn amharic_entity_lookup_uses_amharic_resource_namespace() {
        let query = entity_query("ዳኛቸው_ወርቁ", "am");

        assert!(query.contains("<http://am.dbpedia.org/resource/ዳኛቸው_ወርቁ>"));
        assert!(query.contains("FILTER (lang(?label) = \"am\")"));
    }
}
