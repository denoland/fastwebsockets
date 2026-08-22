use crate::extensions::WebSocketExtensionParams;

#[derive(Debug)]
enum WebSocketExtentionError {
  Unknown,
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum PerMessageDeflateExtensionError {
  InvalidParam,
  Unknown,
}

/// The permessage deflate extension as defined in [RFC 7692](https://datatracker.ietf.org/doc/rfc7692/).
///
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct PermessageDeflateWebSocketExtension {
  pub(crate) server_context_takeover: bool,
  pub(crate) server_max_window_bits: Option<u8>,
  pub(crate) client_context_takeover: bool,

  /// The "client_max_window_bits" Extension Parameter (7.1.2.2)
  /// The parameter has an optional decimal integer value.
  pub(crate) client_max_window_bits: Option<Option<u8>>,
}

pub const PERMESSAGE_DEFLATE: &str = "permessage-deflate";

const SERVER_NO_CONTEXT_TAKEOVER: &str = "server_no_context_takeover";
const CLIENT_NO_CONTEXT_TAKEOVER: &str = "client_no_context_takeover";
const SERVER_MAX_WINDOW_BITS: &str = "server_max_window_bits";
const CLIENT_MAX_WINDOW_BITS: &str = "client_max_window_bits";

impl Default for PermessageDeflateWebSocketExtension {
  fn default() -> Self {
    PermessageDeflateWebSocketExtension {
      server_context_takeover: true,
      server_max_window_bits: None,
      client_context_takeover: true,
      client_max_window_bits: None,
    }
  }
}

impl<'a> TryFrom<&WebSocketExtensionParams<'a>>
  for PermessageDeflateWebSocketExtension
{
  type Error = PerMessageDeflateExtensionError;

  fn try_from(
    params: &WebSocketExtensionParams<'a>,
  ) -> Result<Self, Self::Error> {
    let mut ext_params = Self::default();

    for (param, value) in params {
      match *param {
        SERVER_NO_CONTEXT_TAKEOVER => {
          if value.is_some() {
            return Err(PerMessageDeflateExtensionError::InvalidParam);
          }
          ext_params.server_context_takeover = false;
        }
        CLIENT_NO_CONTEXT_TAKEOVER => {
          if value.is_some() {
            return Err(PerMessageDeflateExtensionError::InvalidParam);
          }
          ext_params.client_context_takeover = false;
        }
        SERVER_MAX_WINDOW_BITS => {
          let bits = value
            .ok_or(PerMessageDeflateExtensionError::InvalidParam)
            .and_then(|v| {
              v.parse::<u8>()
                .map_err(|_| PerMessageDeflateExtensionError::InvalidParam)
            })
            .and_then(|bits| {
              if (8..=15).contains(&bits) {
                Ok(bits)
              } else {
                Err(PerMessageDeflateExtensionError::InvalidParam)
              }
            })?;

          ext_params.server_max_window_bits = Some(bits);
        }
        CLIENT_MAX_WINDOW_BITS => {
          let bits = value
            .map(|value| {
              value
                .parse::<u8>()
                .map_err(|_| PerMessageDeflateExtensionError::InvalidParam)
                .and_then(|bits| {
                  if (8..=15).contains(&bits) {
                    Ok(bits)
                  } else {
                    Err(PerMessageDeflateExtensionError::InvalidParam)
                  }
                })
            })
            .transpose()?;

          ext_params.client_max_window_bits = Some(bits);
        }
        _ => {
          return Err(PerMessageDeflateExtensionError::InvalidParam);
        }
      }
    }

    Ok(ext_params)
  }
}

#[cfg(test)]
mod tests {
  use crate::extensions::WebSocketExtensions;

  use super::*;

  #[test]
  fn basic() {
    let extensions = WebSocketExtensions::from(
            "permessage-deflate; client_max_window_bits; server_max_window_bits=10, permessage-deflate; client_max_window_bits"
        );

    let extensions = Vec::from_iter(extensions.iter());

    assert_eq!(extensions[0].name, "permessage-deflate");
    assert_eq!(
      PermessageDeflateWebSocketExtension::try_from(&extensions[0].params),
      Ok(PermessageDeflateWebSocketExtension {
        server_context_takeover: true,
        server_max_window_bits: Some(10),
        client_context_takeover: true,
        client_max_window_bits: Some(None)
      })
    );

    assert_eq!(extensions[1].name, "permessage-deflate");
    assert_eq!(
      PermessageDeflateWebSocketExtension::try_from(&extensions[1].params),
      Ok(PermessageDeflateWebSocketExtension {
        server_context_takeover: true,
        server_max_window_bits: None,
        client_context_takeover: true,
        client_max_window_bits: Some(None)
      })
    );
  }

  #[test]
  fn server_max_window_bits_no_value() {
    let extensions =
      WebSocketExtensions::from("permessage-deflate; server_max_window_bits");

    let extensions = Vec::from_iter(extensions.iter());

    assert_eq!(extensions[0].name, "permessage-deflate");
    assert_eq!(
      PermessageDeflateWebSocketExtension::try_from(&extensions[0].params),
      Err(PerMessageDeflateExtensionError::InvalidParam),
    );
  }

  #[test]
  fn client_max_window_bits_no_value() {
    let extensions =
      WebSocketExtensions::from("permessage-deflate; client_max_window_bits");

    let extensions = Vec::from_iter(extensions.iter());

    assert_eq!(extensions[0].name, "permessage-deflate");
    assert_eq!(
      PermessageDeflateWebSocketExtension::try_from(&extensions[0].params),
      Ok(PermessageDeflateWebSocketExtension {
        server_context_takeover: true,
        server_max_window_bits: None,
        client_context_takeover: true,
        client_max_window_bits: Some(None)
      })
    );
  }

  #[test]
  fn client_max_window_bits_with_value() {
    let extensions = WebSocketExtensions::from(
      "permessage-deflate; client_max_window_bits=15",
    );

    let extensions = Vec::from_iter(extensions.iter());

    assert_eq!(extensions[0].name, "permessage-deflate");
    assert_eq!(
      PermessageDeflateWebSocketExtension::try_from(&extensions[0].params),
      Ok(PermessageDeflateWebSocketExtension {
        server_context_takeover: true,
        server_max_window_bits: None,
        client_context_takeover: true,
        client_max_window_bits: Some(Some(15))
      })
    );
  }

  #[test]
  fn client_max_window_bits_invalid_value() {
    let extensions = WebSocketExtensions::from(
      "permessage-deflate; client_max_window_bits=16",
    );

    let extensions = Vec::from_iter(extensions.iter());

    assert_eq!(extensions[0].name, "permessage-deflate");
    assert_eq!(
      PermessageDeflateWebSocketExtension::try_from(&extensions[0].params),
      Err(PerMessageDeflateExtensionError::InvalidParam)
    );
  }
}
