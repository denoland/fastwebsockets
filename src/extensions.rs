use std::slice::Iter;

/// Represents a Sec-WebSocket-Extensions segment parameters.
/// The string "client_max_window_bits; server_max_window_bits=10" will turn
/// into `[("client_max_window_bits", None), ("server_max_window_bits", Some("10"))]`
pub type WebSocketExtensionParams<'a> = Vec<(&'a str, Option<&'a str>)>;

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

/// Parses the Sec-WebSocket-Extensions header value.
impl<'a> From<&'a str> for WebSocketExtensions<'a> {
  fn from(value: &'a str) -> Self {
    let extensions = value
      .split(',')
      .filter_map(|extension| {
        let mut extension = extension.split(';');

        extension
          .next()
          .map(|value| value.trim())
          .take_if(|value| !value.is_empty())
          .map(|name| {
            let params = extension
              .filter_map(|value| {
                let mut split = value.splitn(2, '=');

                split
                  .next()
                  .map(|key| key.trim())
                  .take_if(|key| !key.is_empty())
                  .map(|key| {
                    let value = split
                      .next()
                      .map(|value| value.trim())
                      .take_if(|value| !value.is_empty())
                      .map(|value| value);

                    (key, value)
                  })
              })
              .collect::<Vec<_>>();

            WebSocketExtension { name, params }
          })
      })
      .collect::<Vec<_>>();
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
    assert_eq!(WebSocketExtensions::from(""), WebSocketExtensions(vec![]));
    assert_eq!(
      WebSocketExtensions::from("    "),
      WebSocketExtensions(vec![])
    );
    assert_eq!(
      WebSocketExtensions::from(";  ;  "),
      WebSocketExtensions(vec![])
    );
    assert_eq!(
      WebSocketExtensions::from(";  ; ,, "),
      WebSocketExtensions(vec![])
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
}
