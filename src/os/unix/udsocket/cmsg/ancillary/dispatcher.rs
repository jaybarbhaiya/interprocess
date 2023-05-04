use super::{
    credentials::{Credentials, SizeMismatch},
    file_descriptors::FileDescriptors,
    Cmsg, FromCmsg, ParseError, ParseErrorKind, ParseResult, LEVEL,
};
use std::{
    convert::Infallible,
    error::Error,
    fmt::{self, Display, Formatter},
};

/// A dispatch enumeration of all known ancillary message wrapper structs for Ud-sockets.
#[derive(Debug)]
#[non_exhaustive]
#[allow(missing_docs)] // Self-explanatory
pub enum Ancillary<'a> {
    FileDescriptors(FileDescriptors<'a>),
    Credentials(Credentials<'a>),
}
impl<'a> Ancillary<'a> {
    fn parse_fd(cmsg: Cmsg<'a>) -> ParseResult<'a, Self, MalformedPayload> {
        FileDescriptors::try_parse(cmsg)
            .map(Self::FileDescriptors)
            .map_err(|e| e.map_payload_err(MalformedPayload::from))
    }
    fn parse_credentials(cmsg: Cmsg<'a>) -> ParseResult<'a, Self, MalformedPayload> {
        Credentials::try_parse(cmsg)
            .map(Self::Credentials)
            .map_err(|e| e.map_payload_err(MalformedPayload::Credentials))
    }
}
impl<'a> FromCmsg<'a> for Ancillary<'a> {
    type MalformedPayloadError = MalformedPayload;
    fn try_parse(cmsg: Cmsg<'a>) -> ParseResult<'a, Self, MalformedPayload> {
        let (cml, cmt) = (cmsg.cmsg_level(), cmsg.cmsg_type());
        if cml != LEVEL {
            return Err(ParseError {
                cmsg,
                kind: ParseErrorKind::WrongLevel {
                    expected: Some(LEVEL),
                    got: cml,
                },
            });
        }

        // let's get down to jump tables
        match cmsg.cmsg_type() {
            FileDescriptors::TYPE => Self::parse_fd(cmsg),
            Credentials::TYPE => Self::parse_credentials(cmsg),
            _ => Err(ParseError {
                cmsg,
                kind: ParseErrorKind::WrongType {
                    expected: None,
                    got: cmt,
                },
            }),
        }
    }
}

/// Compound error type for [`Ancillary`]'s [`FromCmsg`] implementation.
#[derive(Debug)]
#[allow(missing_docs)] // Self-explanatory
pub enum MalformedPayload {
    Credentials(SizeMismatch),
}
impl Display for MalformedPayload {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials(e) => Display::fmt(e, f),
        }
    }
}
impl Error for MalformedPayload {}
impl From<Infallible> for MalformedPayload {
    fn from(nuh_uh: Infallible) -> Self {
        match nuh_uh {}
    }
}