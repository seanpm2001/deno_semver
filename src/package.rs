// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use std::cmp::Ordering;

use monch::ParseErrorFailure;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use url::Url;

use crate::Version;
use crate::VersionReq;
use crate::VersionReqSpecifierParseError;
use crate::WILDCARD_VERSION_REQ;

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum PackageKind {
  Jsr,
  Npm,
}

impl PackageKind {
  pub fn scheme_with_colon(self) -> &'static str {
    match self {
      Self::Jsr => "jsr:",
      Self::Npm => "npm:",
    }
  }
}

#[derive(Error, Debug, Clone)]
pub enum PackageReqReferenceParseError {
  #[error("Not {} specifier.", .0.scheme_with_colon())]
  NotExpectedScheme(PackageKind),
  #[error(transparent)]
  Invalid(Box<PackageReqReferenceInvalidParseError>),
  #[error(transparent)]
  InvalidPathWithVersion(Box<PackageReqReferenceInvalidWithVersionParseError>),
}

#[derive(Error, Debug, Clone)]
#[error("Invalid package specifier '{specifier}'. {source:#}")]
pub struct PackageReqReferenceInvalidParseError {
  specifier: String,
  #[source]
  source: PackageReqPartsParseError,
}

#[derive(Error, Debug, Clone)]
#[error("Invalid package specifier '{0}{1}'. Did you mean to write '{0}{2}'?", .kind.scheme_with_colon(), current, suggested)]
pub struct PackageReqReferenceInvalidWithVersionParseError {
  kind: PackageKind,
  current: String,
  suggested: String,
}

/// A reference to a package's name, version constraint, and potential sub path.
///
/// This contains all the information found in a package specifier other than
/// what kind of package specifier it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageReqReference {
  pub req: PackageReq,
  pub sub_path: Option<String>,
}

impl PackageReqReference {
  #[allow(clippy::should_implement_trait)]
  pub(crate) fn from_str(
    specifier: &str,
    kind: PackageKind,
  ) -> Result<Self, PackageReqReferenceParseError> {
    let original_text = specifier;
    let input = match specifier.strip_prefix(kind.scheme_with_colon()) {
      Some(input) => input,
      None => {
        // this is hit a lot when a url is not the expected scheme
        // so ensure nothing heavy occurs before this
        return Err(PackageReqReferenceParseError::NotExpectedScheme(kind));
      }
    };
    let (req, sub_path) = match PackageReq::parse_with_path(input) {
      Ok(pkg_req) => pkg_req,
      Err(err) => {
        return Err(PackageReqReferenceParseError::Invalid(Box::new(
          PackageReqReferenceInvalidParseError {
            specifier: original_text.to_string(),
            source: err,
          },
        )));
      }
    };
    let sub_path = if sub_path.is_empty() || sub_path == "/" {
      None
    } else {
      Some(sub_path.to_string())
    };

    if let Some(sub_path) = &sub_path {
      if let Some(at_index) = sub_path.rfind('@') {
        let (new_sub_path, version) = sub_path.split_at(at_index);
        return Err(PackageReqReferenceParseError::InvalidPathWithVersion(
          Box::new(PackageReqReferenceInvalidWithVersionParseError {
            kind,
            current: format!("{req}/{sub_path}"),
            suggested: format!("{req}{version}/{new_sub_path}"),
          }),
        ));
      }
    }

    Ok(Self { req, sub_path })
  }
}

impl std::fmt::Display for PackageReqReference {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if let Some(sub_path) = &self.sub_path {
      write!(f, "{}/{}", self.req, sub_path)
    } else {
      write!(f, "{}", self.req)
    }
  }
}

#[derive(Error, Debug, Clone)]
pub enum PackageReqPartsParseError {
  #[error("Did not contain a package name.")]
  NoPackageName,
  #[error("Did not contain a valid package name.")]
  InvalidPackageName,
  #[error("Invalid version requirement. {source:#}")]
  VersionReq {
    #[source]
    source: VersionReqSpecifierParseError,
  },
}

#[derive(Error, Debug, Clone)]
#[error("Invalid package requirement '{text}'. {source:#}")]
pub struct PackageReqParseError {
  pub text: String,
  #[source]
  pub source: PackageReqPartsParseError,
}

/// The name and version constraint component of an `PackageReqReference`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PackageReq {
  pub name: String,
  pub version_req: VersionReq,
}

impl std::fmt::Display for PackageReq {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if self.version_req.version_text() == "*" {
      // do not write out the version requirement when it's the wildcard version
      write!(f, "{}", self.name)
    } else {
      write!(f, "{}@{}", self.name, self.version_req)
    }
  }
}

impl PackageReq {
  #[allow(clippy::should_implement_trait)]
  pub fn from_str(text: &str) -> Result<Self, PackageReqParseError> {
    fn from_str_inner(
      text: &str,
    ) -> Result<PackageReq, PackageReqPartsParseError> {
      let (req, path) = PackageReq::parse_with_path(text)?;
      if !path.is_empty() {
        return Err(PackageReqPartsParseError::VersionReq {
          source: VersionReqSpecifierParseError {
            source: ParseErrorFailure::new(
              &text[text.len() - path.len() - 1..],
              "Unexpected character '/'",
            )
            .into_error(),
          },
        });
      }
      Ok(req)
    }

    match from_str_inner(text) {
      Ok(req) => Ok(req),
      Err(err) => Err(PackageReqParseError {
        text: text.to_string(),
        source: err,
      }),
    }
  }

  fn parse_with_path(
    input: &str,
  ) -> Result<(Self, &str), PackageReqPartsParseError> {
    // Strip leading slash, which might come from import map
    let input = input.strip_prefix('/').unwrap_or(input);
    // parse the first name part
    let (first_part, input) = input.split_once('/').unwrap_or((input, ""));
    if first_part.is_empty() {
      return Err(PackageReqPartsParseError::NoPackageName);
    }
    // if it starts with an @, parse the second name part
    let (maybe_scope, last_name_part, sub_path) = if first_part.starts_with('@')
    {
      let (second_part, input) = input.split_once('/').unwrap_or((input, ""));
      if second_part.is_empty() {
        return Err(PackageReqPartsParseError::InvalidPackageName);
      }
      (Some(first_part), second_part, input)
    } else {
      (None, first_part, input)
    };

    let (last_name_part, version_req) = if let Some((last_name_part, version)) =
      last_name_part.rsplit_once('@')
    {
      let version_req = VersionReq::parse_from_specifier(version)
        .map_err(|err| PackageReqPartsParseError::VersionReq { source: err })?;
      (last_name_part, Some(version_req))
    } else {
      (last_name_part, None)
    };
    Ok((
      Self {
        name: match maybe_scope {
          Some(scope) => format!("{}/{}", scope, last_name_part),
          None => last_name_part.to_string(),
        },
        version_req: version_req
          .unwrap_or_else(|| WILDCARD_VERSION_REQ.clone()),
      },
      sub_path,
    ))
  }
}

impl Serialize for PackageReq {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: serde::Serializer,
  {
    serializer.serialize_str(&self.to_string())
  }
}

impl<'de> Deserialize<'de> for PackageReq {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    let text = String::deserialize(deserializer)?;
    match Self::from_str(&text) {
      Ok(req) => Ok(req),
      Err(err) => Err(serde::de::Error::custom(err)),
    }
  }
}

impl PartialOrd for PackageReq {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
}

// Sort the package requirements alphabetically then the version
// requirement in a way that will lead to the least number of
// duplicate packages (so sort None last since it's `*`), but
// mostly to create some determinism around how these are resolved.
impl Ord for PackageReq {
  fn cmp(&self, other: &Self) -> Ordering {
    fn cmp_specifier_version_req(a: &VersionReq, b: &VersionReq) -> Ordering {
      match a.tag() {
        Some(a_tag) => match b.tag() {
          Some(b_tag) => b_tag.cmp(a_tag), // sort descending
          None => Ordering::Less,          // prefer a since tag
        },
        None => {
          match b.tag() {
            Some(_) => Ordering::Greater, // prefer b since tag
            None => {
              // At this point, just sort by text descending.
              // We could maybe be a bit smarter here in the future.
              b.to_string().cmp(&a.to_string())
            }
          }
        }
      }
    }

    match self.name.cmp(&other.name) {
      Ordering::Equal => {
        cmp_specifier_version_req(&self.version_req, &other.version_req)
      }
      ordering => ordering,
    }
  }
}

#[derive(Debug, Error, Clone)]
#[error("Invalid package name and version reference '{text}'. {message}")]
pub struct PackageNvReferenceParseError {
  pub message: String,
  pub text: String,
}

/// A package name and version with a potential subpath.
#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct PackageNvReference {
  pub nv: PackageNv,
  pub sub_path: Option<String>,
}

impl PackageNvReference {
  #[allow(clippy::should_implement_trait)]
  pub(crate) fn from_str(
    nv: &str,
    kind: PackageKind,
  ) -> Result<Self, PackageNvReferenceParseError> {
    use monch::*;

    fn sub_path(input: &str) -> ParseResult<&str> {
      let (input, _) = ch('/')(input)?;
      Ok(("", input))
    }

    fn parse_ref<'a>(
      kind: PackageKind,
    ) -> impl Fn(&'a str) -> ParseResult<'a, PackageNvReference> {
      move |input| {
        let (input, _) = tag(kind.scheme_with_colon())(input)?;
        let (input, _) = maybe(ch('/'))(input)?;
        let (input, nv) = parse_nv(input)?;
        let (input, maybe_sub_path) = maybe(sub_path)(input)?;
        Ok((
          input,
          PackageNvReference {
            nv,
            sub_path: maybe_sub_path.map(ToOwned::to_owned),
          },
        ))
      }
    }

    with_failure_handling(parse_ref(kind))(nv).map_err(|err| {
      PackageNvReferenceParseError {
        message: format!("{err:#}"),
        text: nv.to_string(),
      }
    })
  }

  pub(crate) fn as_specifier(&self, kind: PackageKind) -> Url {
    let version_text = self.nv.version.to_string();
    let scheme_with_colon = kind.scheme_with_colon();
    let capacity = scheme_with_colon.len() + 1 /* slash */
      + self.nv.name.len()
      + 1 /* @ */
      + version_text.len()
      + self.sub_path.as_ref().map(|p| p.len() + 1 /* slash */).unwrap_or(0);
    let mut text = String::with_capacity(capacity);
    text.push_str(scheme_with_colon);
    text.push('/');
    text.push_str(&self.nv.name);
    text.push('@');
    text.push_str(&version_text);
    if let Some(sub_path) = &self.sub_path {
      text.push('/');
      text.push_str(sub_path);
    }
    debug_assert_eq!(text.len(), capacity);
    Url::parse(&text).unwrap()
  }
}

impl std::fmt::Display for PackageNvReference {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if let Some(sub_path) = &self.sub_path {
      write!(f, "{}/{}", self.nv, sub_path)
    } else {
      write!(f, "{}", self.nv)
    }
  }
}

#[derive(Debug, Error, Clone)]
#[error("Invalid package name and version '{text}'. {message}")]
pub struct PackageNvParseError {
  pub message: String,
  pub text: String,
}

#[derive(Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct PackageNv {
  pub name: String,
  pub version: Version,
}

impl std::fmt::Debug for PackageNv {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    // when debugging, it's easier to compare this
    write!(f, "{}@{}", self.name, self.version)
  }
}

impl std::fmt::Display for PackageNv {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}@{}", self.name, self.version)
  }
}

impl Serialize for PackageNv {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: serde::Serializer,
  {
    serializer.serialize_str(&self.to_string())
  }
}

impl<'de> Deserialize<'de> for PackageNv {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    let text = String::deserialize(deserializer)?;
    match Self::from_str(&text) {
      Ok(req) => Ok(req),
      Err(err) => Err(serde::de::Error::custom(err)),
    }
  }
}

impl PackageNv {
  #[allow(clippy::should_implement_trait)]
  pub fn from_str(nv: &str) -> Result<Self, PackageNvParseError> {
    monch::with_failure_handling(parse_nv)(nv).map_err(|err| {
      PackageNvParseError {
        message: format!("{err:#}"),
        text: nv.to_string(),
      }
    })
  }

  pub fn scope(&self) -> Option<&str> {
    if self.name.starts_with('@') {
      self.name.split('/').next()
    } else {
      None
    }
  }
}

fn parse_nv(input: &str) -> monch::ParseResult<PackageNv> {
  use monch::*;

  fn parse_name(input: &str) -> ParseResult<&str> {
    if_not_empty(substring(move |input| {
      for (pos, c) in input.char_indices() {
        // first character might be a scope, so skip it
        if pos > 0 && c == '@' {
          return Ok((&input[pos..], ()));
        }
      }
      ParseError::backtrace()
    }))(input)
  }

  fn parse_version(input: &str) -> ParseResult<&str> {
    if_not_empty(substring(skip_while(|c| !matches!(c, '_' | '/'))))(input)
  }

  let (input, name) = parse_name(input)?;
  let (input, _) = ch('@')(input)?;
  let at_version_input = input;
  let (input, version) = parse_version(input)?;
  match Version::parse_from_npm(version) {
    Ok(version) => Ok((
      input,
      PackageNv {
        name: name.to_string(),
        version,
      },
    )),
    Err(err) => ParseError::fail(at_version_input, format!("{err:#}")),
  }
}

#[cfg(test)]
mod test {
  use std::cmp::Ordering;

  use crate::package::PackageReq;

  #[test]
  fn serialize_deserialize_package_req() {
    let package_req = PackageReq::from_str("test@^1.0").unwrap();
    let json = serde_json::to_string(&package_req).unwrap();
    assert_eq!(json, "\"test@^1.0\"");
    let result = serde_json::from_str::<PackageReq>(&json).unwrap();
    assert_eq!(result, package_req);
  }

  #[test]
  fn sorting_package_reqs() {
    fn cmp_req(a: &str, b: &str) -> Ordering {
      let a = PackageReq::from_str(a).unwrap();
      let b = PackageReq::from_str(b).unwrap();
      a.cmp(&b)
    }

    // sort by name
    assert_eq!(cmp_req("a", "b@1"), Ordering::Less);
    assert_eq!(cmp_req("b@1", "a"), Ordering::Greater);
    // prefer non-wildcard
    assert_eq!(cmp_req("a", "a@1"), Ordering::Greater);
    assert_eq!(cmp_req("a@1", "a"), Ordering::Less);
    // prefer tag
    assert_eq!(cmp_req("a@tag", "a"), Ordering::Less);
    assert_eq!(cmp_req("a", "a@tag"), Ordering::Greater);
    // sort tag descending
    assert_eq!(cmp_req("a@latest-v1", "a@latest-v2"), Ordering::Greater);
    assert_eq!(cmp_req("a@latest-v2", "a@latest-v1"), Ordering::Less);
    // sort version req descending
    assert_eq!(cmp_req("a@1", "a@2"), Ordering::Greater);
    assert_eq!(cmp_req("a@2", "a@1"), Ordering::Less);
  }
}
