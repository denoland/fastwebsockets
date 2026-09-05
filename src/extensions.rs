use std::{slice::Iter, str::Split};

/// (key: &str, value: Option<&str>)
pub type ExtensionParam<'a> = (&'a str, Option<&'a str>);

/// Represents a Sec-WebSocket-Extensions segment parameters.
/// The string "client_max_window_bits; server_max_window_bits=10" will turn
/// into `[("client_max_window_bits", None), ("server_max_window_bits", Some("10"))]`
pub type WebSocketExtensionParams<'a> = Vec<ExtensionParam<'a>>;

/// Combination of extension name and params.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct WebSocketExtension<'a> {
  pub(crate) name: &'a str,
  pub(crate) params: WebSocketExtensionParams<'a>,
}

/// List of extensions and their respective parameters defined in a Sec-WebSocket-Extensions header
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct WebSocketExtensions<'a>(Vec<WebSocketExtension<'a>>);

fn extension_params<'a>(input: Split<'a, char>) -> Vec<ExtensionParam<'a>> {
  input
    .filter_map(|value| {
      let mut split = value.splitn(2, '=');

      split.next().map(|key| {
        let value = split.next().map(|value| value.trim());

        (key.trim(), value)
      })
    })
    .collect::<Vec<_>>()
}

fn extension<'a>(input: &'a str) -> Option<WebSocketExtension<'a>> {
  let mut extension = input.split(';');

  extension.next().map(|name| {
    let params = extension_params(extension);
    WebSocketExtension {
      name: name.trim(),
      params,
    }
  })
}

/// Parses the Sec-WebSocket-Extensions header value.
impl<'a> From<&'a str> for WebSocketExtensions<'a> {
  fn from(value: &'a str) -> Self {
    let extensions = value.split(',').filter_map(extension).collect::<Vec<_>>();
    Self(extensions)
  }
}

impl<'a> WebSocketExtensions<'a> {
  pub fn iter(&'a self) -> Iter<'a, WebSocketExtension<'a>> {
    self.0.iter()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn empty() {
    assert_eq!(
      WebSocketExtensions::from(""),
      WebSocketExtensions(vec![WebSocketExtension {
        name: "",
        params: vec![]
      }])
    );
    assert_eq!(
      WebSocketExtensions::from("    "),
      WebSocketExtensions(vec![WebSocketExtension {
        name: "",
        params: vec![]
      }])
    );
    assert_eq!(
      WebSocketExtensions::from(";  ;  "),
      WebSocketExtensions(vec![WebSocketExtension {
        name: "",
        params: vec![("", None), ("", None)]
      }])
    );
    assert_eq!(
      WebSocketExtensions::from(";  ; ,, "),
      WebSocketExtensions(vec![
        WebSocketExtension {
          name: "",
          params: vec![("", None), ("", None)]
        },
        WebSocketExtension {
          name: "",
          params: vec![]
        },
        WebSocketExtension {
          name: "",
          params: vec![]
        }
      ])
    );
  }

  #[test]
  fn basic() {
    assert_eq!(
            WebSocketExtensions::from(
                "permessage-deflate; client_max_window_bits; server_max_window_bits=10, permessage-deflate; client_max_window_bits"
            ),
            WebSocketExtensions(vec![
                WebSocketExtension {
                    name: "permessage-deflate",
                    params: vec![
                        ("client_max_window_bits", None),
                        ("server_max_window_bits", Some("10"))
                    ]
                },
                WebSocketExtension {
                    name: "permessage-deflate",
                    params: vec![("client_max_window_bits", None)]
                }
            ])
        );
  }

  #[test]
  fn empty_param_pair() {
    assert_eq!(
            WebSocketExtensions::from(
                "permessage-deflate; client_max_window_bits; =, permessage-deflate; client_max_window_bits"
            ),
            WebSocketExtensions(vec![
                WebSocketExtension {
                    name: "permessage-deflate",
                    params: vec![
                        ("client_max_window_bits", None),
                        ("", Some(""))
                    ]
                },
                WebSocketExtension {
                    name: "permessage-deflate",
                    params: vec![("client_max_window_bits", None)]
                }
            ])
        );
  }
}
