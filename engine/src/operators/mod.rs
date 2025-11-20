use crate::error::{Error, Result, new_regex_error};
use crate::info::Version;
use crate::operators::extractors::{Extractor, ExtractorType};
use crate::operators::matchers::{Condition, FaviconMap, Matcher, MatcherType};
use crate::operators::target::OperatorTarget;
use crate::serde_format::is_default;
use serde::{Deserialize, Serialize};
use slinger::Response;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

pub mod extractors;
pub mod matchers;
pub mod regex;
pub mod target;

/// Operators for the current request go here.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct Operators {
  // description: |
  //   StopAtFirstMatch stops the execution of the requests and template as soon as a match is found.
  #[serde(default, skip_serializing_if = "is_default")]
  #[cfg_attr(
    feature = "mcp",
    schemars(
      title = "stop at first match",
      description = "Stop the execution after a match is found"
    )
  )]
  pub stop_at_first_match: bool,
  // description: |
  //   MatchersCondition is the condition between the matchers. Default is OR.
  // values:
  //   - "and"
  //   - "or"
  #[serde(default, skip_serializing_if = "is_default")]
  #[cfg_attr(
    feature = "mcp",
    schemars(
      title = "condition between the matchers",
      description = "Conditions between the matchers",
    )
  )]
  pub matchers_condition: Condition,
  // description: |
  //   Matchers contains the detection mechanism for the request to identify
  //   whether the request was successful by doing pattern matching
  //   on request/responses.
  //
  //   Multiple matchers can be combined with `matcher-condition` flag
  //   which accepts either `and` or `or` as argument.
  #[serde(default, skip_serializing_if = "is_default")]
  #[cfg_attr(
    feature = "mcp",
    schemars(
      title = "matchers to run on response",
      description = "Detection mechanism to identify whether the request was successful by doing pattern matching"
    )
  )]
  pub matchers: Vec<Arc<Matcher>>,
  // description: |
  //   Extractors contains the extraction mechanism for the request to identify
  //   and extract parts of the response.
  #[serde(default, skip_serializing_if = "is_default")]
  #[cfg_attr(
    feature = "mcp",
    schemars(
      title = "extractors to run on response",
      description = "Extractors contains the extraction mechanism for the request to identify and extract parts of the response"
    )
  )]
  pub extractors: Vec<Arc<Extractor>>,
}

impl Operators {
  pub fn compile(&mut self) -> Result<()> {
    for matcher in self.matchers.iter_mut() {
      let mutable_matcher = Arc::make_mut(matcher);
      mutable_matcher.compile().map_err(new_regex_error)?;
    }
    for extractor in self.extractors.iter_mut() {
      let mutable_extractor = Arc::make_mut(extractor);
      mutable_extractor.compile().map_err(new_regex_error)?;
    }
    Ok(())
  }
  
  /// Generic extractor that works with any OperatorTarget (Response or Request)
  pub fn extractor_generic<T: OperatorTarget>(
    &self,
    version: Option<Version>,
    target: &T,
    result: &mut OperatorResult,
  ) {
    for (index, extractor) in self.extractors.iter().enumerate() {
      let (words, body) =
        if let Ok((words, body)) = extractor.part.get_matcher_word_from_part(target) {
          (words, body)
        } else {
          continue;
        };
      let (extract_result, version) = match &extractor.extractor_type {
        ExtractorType::Regex(re) => extractor.extract_regex(re, words, body, &version),
        ExtractorType::JSON(json) => extractor.extract_json(json, words),
        ExtractorType::KVal(..) | ExtractorType::XPath(..) | ExtractorType::DSL(..) => {
          (HashSet::new(), BTreeMap::new())
        }
      };
      if !extract_result.is_empty() {
        let key = extractor.name.clone().unwrap_or(index.to_string());
        if let Some(er) = result.extract_result.get_mut(&key) {
          er.extend(extract_result);
        } else {
          result.extract_result.insert(key.clone(), extract_result);
        }
      }
      for (k, v) in version {
        result.extract_result.insert(k, HashSet::from_iter([v]));
      }
    }
  }
  
  pub fn extractor(
    &self,
    version: Option<Version>,
    response: &Response,
    result: &mut OperatorResult,
  ) {
    self.extractor_generic(version, response, result)
  }
  
  /// Generic matcher that works with any OperatorTarget (Response or Request)
  /// For Response, it can access extensions for favicon and status code
  /// For Request, status code matching will be skipped
  pub fn matcher_generic<T: OperatorTarget>(
    &self,
    target: &T,
    response_for_extensions: Option<&Response>,
    result: &mut OperatorResult,
  ) -> Result<()> {
    let mut matchers = Vec::new();
    if self.matchers.is_empty() {
      return Ok(());
    }
    for matcher in self.matchers.iter() {
      // extract matcher word from target parts
      let (words, body) = matcher.part.get_matcher_word_from_part(target)?;
      let (is_match, mw) = match &matcher.matcher_type {
        MatcherType::Word(word) => matcher.match_word(word, words),
        MatcherType::Favicon(fav) => {
          // Favicon matching requires response extensions
          if let Some(response) = response_for_extensions {
            let hm = response
              .extensions()
              .get::<BTreeMap<String, FaviconMap>>()
              .ok_or(Error::IO(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "not found favicon",
              )))?;
            matcher.match_favicon(fav, hm)
          } else {
            (false, Vec::new())
          }
        }
        MatcherType::Status(status) => {
          // Status code matching only works for Response
          if let Some(response) = response_for_extensions {
            (
              matcher.match_status_code(status, response.status_code().as_u16()),
              vec![response.status_code().as_u16().to_string()],
            )
          } else {
            (false, Vec::new())
          }
        }
        MatcherType::Regex(re) => matcher.match_regex(re, words, body),
        MatcherType::None
        | MatcherType::DSL(..)
        | MatcherType::Binary(..)
        | MatcherType::XPath(..) => (false, Vec::new()),
      };
      // normalize negative match
      let is_match = matcher.negative(is_match);
      matchers.push(is_match);
      if !is_match {
        match self.matchers_condition {
          Condition::Or => continue,
          Condition::And => {
            result.matched = false;
            break;
          }
        }
      } else {
        if let Some(name) = &matcher.name {
          result.name.insert(name.clone());
        }
        result.matcher_word.extend(mw);
        if matches!(self.matchers_condition, Condition::Or) {
          result.matched = true;
          if self.stop_at_first_match {
            break;
          }
        }
      }
    }
    if matches!(self.matchers_condition, Condition::And) && matchers.iter().all(|x| *x) {
      result.matched = true;
    }
    Ok(())
  }
  
  /// 匹配接口统一为只接收 &Response，request 可通过 response.extensions().get::<Request>() 访问
  pub fn matcher(&self, response: &Response, result: &mut OperatorResult) -> Result<()> {
    self.matcher_generic(response, Some(response), result)
  }
}

#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct OperatorResult {
  /// Description: Indicates whether the template matched the response
  /// Example: true
  #[cfg_attr(
    feature = "mcp",
    schemars(
      title = "Match Status",
      description = "Boolean indicating if the template matched the response",
      example = "true"
    )
  )]
  matched: bool,
  /// Description: Set of names that matched during the operation
  /// Example: ["apache", "tomcat"]
  #[cfg_attr(
    feature = "mcp",
    schemars(
      title = "Matched Names",
      description = "Set of names that matched during the operation",
      example = r#"["apache", "tomcat"]"#
    )
  )]
  name: HashSet<String>,
  /// Description: List of words that triggered the matcher
  /// Example: ["server: apache", "x-powered-by: tomcat"]
  #[cfg_attr(
    feature = "mcp",
    schemars(
      title = "Matcher Words",
      description = "List of words that triggered the matcher",
      example = r#"["server: apache", "x-powered-by: tomcat"]"#
    )
  )]
  matcher_word: Vec<String>,
  /// Description: Key-value pairs of extracted data from the operation
  /// Example: {"user": ["admin"], "version": ["1.0"]}
  #[cfg_attr(
    feature = "mcp",
    schemars(
      title = "Extracted Results",
      description = "Key-value pairs of extracted data from the operation",
      example = r#"{"user": ["admin"], "version": ["1.0"]}"#
    )
  )]
  extract_result: BTreeMap<String, HashSet<String>>,
}

impl OperatorResult {
  pub fn is_matched(&self) -> bool {
    self.matched
  }
  pub fn is_extract(&self) -> bool {
    !self.extract_result.is_empty()
  }
  fn name(&self) -> Vec<String> {
    Vec::from_iter(&self.name)
      .iter()
      .map(|x| x.to_string())
      .collect()
  }
  pub fn matcher_word(&self) -> Vec<String> {
    let mut name = self.matcher_word.clone();
    name.extend(self.name());
    name
  }
  pub fn extract_result(&self) -> BTreeMap<String, HashSet<String>> {
    self.extract_result.clone()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::operators::matchers::{Matcher, MatcherType, Part, Word};
  use slinger::{Request, Response};

  #[test]
  fn test_operator_target_trait_for_request() {
    // Create a mock request
    let request = Request::builder()
      .method("GET")
      .uri("http://example.com/test")
      .header("User-Agent", "test-agent")
      .header("Custom-Header", "custom-value")
      .body(slinger::Body::from("test body"))
      .unwrap();
    let request = Request::from(request);

    // Test get_headers
    let headers = request.get_headers();
    assert!(headers.contains("user-agent: test-agent"));
    assert!(headers.contains("custom-header: custom-value"));

    // Test get_body
    assert!(request.get_body().is_some());

    // Test get_header
    assert_eq!(
      request.get_header("user-agent"),
      Some("test-agent".to_string())
    );
    assert_eq!(
      request.get_header("custom-header"),
      Some("custom-value".to_string())
    );
  }

  #[test]
  fn test_operator_target_trait_for_response() {
    // Create a mock response
    let response = slinger::http::Response::builder()
      .status(200)
      .header("Server", "test-server")
      .header("Content-Type", "text/html")
      .body(slinger::Body::from("<html>test</html>"))
      .unwrap();
    let response = Response::from(response);

    // Test get_headers
    let headers = response.get_headers();
    assert!(headers.contains("server: test-server"));
    assert!(headers.contains("content-type: text/html"));

    // Test get_body
    assert!(response.get_body().is_some());

    // Test get_header
    assert_eq!(response.get_header("server"), Some("test-server".to_string()));
  }

  #[test]
  fn test_matcher_generic_with_request() {
    // Create a request with specific content
    let request = Request::builder()
      .method("POST")
      .uri("http://example.com/api")
      .header("Authorization", "Bearer token123")
      .body(slinger::Body::from("username=admin&password=test"))
      .unwrap();
    let request = Request::from(request);

    // Create a word matcher for the body
    let mut matcher = Matcher {
      matcher_type: MatcherType::Word(Word {
        words: vec!["username=admin".to_string()],
      }),
      name: Some("admin-login".to_string()),
      part: Part::Body,
      ..Default::default()
    };
    matcher.compile().unwrap();

    // Create operators with the matcher
    let operators = Operators {
      matchers: vec![Arc::new(matcher)],
      ..Default::default()
    };

    // Match against the request
    let mut result = OperatorResult::default();
    operators
      .matcher_generic(&request, None, &mut result)
      .unwrap();

    // Verify the match
    assert!(result.is_matched());
    assert!(result.name.contains("admin-login"));
  }

  #[test]
  fn test_matcher_generic_with_response() {
    // Create a response with specific content
    let response = slinger::http::Response::builder()
      .status(200)
      .header("Server", "Apache/2.4.41")
      .body(slinger::Body::from("<html><title>Apache Test</title></html>"))
      .unwrap();
    let response = Response::from(response);

    // Create a word matcher for the header
    let mut matcher = Matcher {
      matcher_type: MatcherType::Word(Word {
        words: vec!["Apache".to_string()],
      }),
      name: Some("apache-server".to_string()),
      part: Part::Header,
      ..Default::default()
    };
    matcher.compile().unwrap();

    // Create operators with the matcher
    let operators = Operators {
      matchers: vec![Arc::new(matcher)],
      ..Default::default()
    };

    // Match against the response
    let mut result = OperatorResult::default();
    operators
      .matcher_generic(&response, Some(&response), &mut result)
      .unwrap();

    // Verify the match
    assert!(result.is_matched());
    assert!(result.name.contains("apache-server"));
  }
}
