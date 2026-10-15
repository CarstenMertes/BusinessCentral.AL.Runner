//! The library types whose values are opaque handles (`JsonObject`, `TextBuilder`, ...), by kind (ADR-032 decision 4).
//!
//! A name the table does not list is kept as it was written in [`LibraryKind::Other`], so a type the runtime does
//! not know yet behaves as before. Deref gives the canonical name, for the places that print it or compare it as
//! text; code that decides by kind matches the variant.

use std::fmt;
use std::ops::Deref;

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LibraryKind {
    BigText,
    Blob,
    CompanyProperty,
    Cookie,
    DataTransfer,
    Debugger,
    Dialog,
    ErrorInfo,
    FieldRef,
    File,
    FileUpload,
    FilterPageBuilder,
    HttpClient,
    HttpContent,
    HttpHeaders,
    HttpRequestMessage,
    HttpResponseMessage,
    InStream,
    IsolatedStorage,
    JsonArray,
    JsonObject,
    JsonToken,
    JsonValue,
    KeyRef,
    Media,
    MediaSet,
    ModuleDependencyInfo,
    ModuleInfo,
    NavApp,
    Notification,
    NumberSequence,
    OutStream,
    RecordId,
    RecordRef,
    SecretText,
    SessionInformation,
    SessionSettings,
    TaskScheduler,
    TestHttpRequestMessage,
    TestHttpResponseMessage,
    TextBuilder,
    Version,
    WebServiceActionContext,
    XmlAttribute,
    XmlAttributeCollection,
    XmlCData,
    XmlComment,
    XmlDeclaration,
    XmlDocument,
    XmlDocumentType,
    XmlElement,
    XmlNamespaceManager,
    XmlNameTable,
    XmlNode,
    XmlNodeList,
    XmlProcessingInstruction,
    XmlText,
    /// A name not in the list above, as written.
    Other(String),
}

impl LibraryKind {
    /// The kind a type name stands for (case does not matter, as in AL).
    pub fn from_name(name: &str) -> LibraryKind {
        match name.to_ascii_lowercase().as_str() {
            "bigtext" => LibraryKind::BigText,
            "blob" => LibraryKind::Blob,
            "companyproperty" => LibraryKind::CompanyProperty,
            "cookie" => LibraryKind::Cookie,
            "datatransfer" => LibraryKind::DataTransfer,
            "debugger" => LibraryKind::Debugger,
            "dialog" => LibraryKind::Dialog,
            "errorinfo" => LibraryKind::ErrorInfo,
            "fieldref" => LibraryKind::FieldRef,
            "file" => LibraryKind::File,
            "fileupload" => LibraryKind::FileUpload,
            "filterpagebuilder" => LibraryKind::FilterPageBuilder,
            "httpclient" => LibraryKind::HttpClient,
            "httpcontent" => LibraryKind::HttpContent,
            "httpheaders" => LibraryKind::HttpHeaders,
            "httprequestmessage" => LibraryKind::HttpRequestMessage,
            "httpresponsemessage" => LibraryKind::HttpResponseMessage,
            "instream" => LibraryKind::InStream,
            "isolatedstorage" => LibraryKind::IsolatedStorage,
            "jsonarray" => LibraryKind::JsonArray,
            "jsonobject" => LibraryKind::JsonObject,
            "jsontoken" => LibraryKind::JsonToken,
            "jsonvalue" => LibraryKind::JsonValue,
            "keyref" => LibraryKind::KeyRef,
            "media" => LibraryKind::Media,
            "mediaset" => LibraryKind::MediaSet,
            "moduledependencyinfo" => LibraryKind::ModuleDependencyInfo,
            "moduleinfo" => LibraryKind::ModuleInfo,
            "navapp" => LibraryKind::NavApp,
            "notification" => LibraryKind::Notification,
            "numbersequence" => LibraryKind::NumberSequence,
            "outstream" => LibraryKind::OutStream,
            "recordid" => LibraryKind::RecordId,
            "recordref" => LibraryKind::RecordRef,
            "secrettext" => LibraryKind::SecretText,
            "sessioninformation" => LibraryKind::SessionInformation,
            "sessionsettings" => LibraryKind::SessionSettings,
            "taskscheduler" => LibraryKind::TaskScheduler,
            "testhttprequestmessage" => LibraryKind::TestHttpRequestMessage,
            "testhttpresponsemessage" => LibraryKind::TestHttpResponseMessage,
            "textbuilder" => LibraryKind::TextBuilder,
            "version" => LibraryKind::Version,
            "webserviceactioncontext" => LibraryKind::WebServiceActionContext,
            "xmlattribute" => LibraryKind::XmlAttribute,
            "xmlattributecollection" => LibraryKind::XmlAttributeCollection,
            "xmlcdata" => LibraryKind::XmlCData,
            "xmlcomment" => LibraryKind::XmlComment,
            "xmldeclaration" => LibraryKind::XmlDeclaration,
            "xmldocument" => LibraryKind::XmlDocument,
            "xmldocumenttype" => LibraryKind::XmlDocumentType,
            "xmlelement" => LibraryKind::XmlElement,
            "xmlnamespacemanager" => LibraryKind::XmlNamespaceManager,
            "xmlnametable" => LibraryKind::XmlNameTable,
            "xmlnode" => LibraryKind::XmlNode,
            "xmlnodelist" => LibraryKind::XmlNodeList,
            "xmlprocessinginstruction" => LibraryKind::XmlProcessingInstruction,
            "xmltext" => LibraryKind::XmlText,
            _ => LibraryKind::Other(name.to_string()),
        }
    }

    /// The canonical name (as `name`, for the places that read a `String`).
    pub fn as_str(&self) -> &str {
        self.name()
    }

    /// The canonical name.
    pub fn name(&self) -> &str {
        match self {
            LibraryKind::BigText => "BigText",
            LibraryKind::Blob => "Blob",
            LibraryKind::CompanyProperty => "CompanyProperty",
            LibraryKind::Cookie => "Cookie",
            LibraryKind::DataTransfer => "DataTransfer",
            LibraryKind::Debugger => "Debugger",
            LibraryKind::Dialog => "Dialog",
            LibraryKind::ErrorInfo => "ErrorInfo",
            LibraryKind::FieldRef => "FieldRef",
            LibraryKind::File => "File",
            LibraryKind::FileUpload => "FileUpload",
            LibraryKind::FilterPageBuilder => "FilterPageBuilder",
            LibraryKind::HttpClient => "HttpClient",
            LibraryKind::HttpContent => "HttpContent",
            LibraryKind::HttpHeaders => "HttpHeaders",
            LibraryKind::HttpRequestMessage => "HttpRequestMessage",
            LibraryKind::HttpResponseMessage => "HttpResponseMessage",
            LibraryKind::InStream => "InStream",
            LibraryKind::IsolatedStorage => "IsolatedStorage",
            LibraryKind::JsonArray => "JsonArray",
            LibraryKind::JsonObject => "JsonObject",
            LibraryKind::JsonToken => "JsonToken",
            LibraryKind::JsonValue => "JsonValue",
            LibraryKind::KeyRef => "KeyRef",
            LibraryKind::Media => "Media",
            LibraryKind::MediaSet => "MediaSet",
            LibraryKind::ModuleDependencyInfo => "ModuleDependencyInfo",
            LibraryKind::ModuleInfo => "ModuleInfo",
            LibraryKind::NavApp => "NavApp",
            LibraryKind::Notification => "Notification",
            LibraryKind::NumberSequence => "NumberSequence",
            LibraryKind::OutStream => "OutStream",
            LibraryKind::RecordId => "RecordId",
            LibraryKind::RecordRef => "RecordRef",
            LibraryKind::SecretText => "SecretText",
            LibraryKind::SessionInformation => "SessionInformation",
            LibraryKind::SessionSettings => "SessionSettings",
            LibraryKind::TaskScheduler => "TaskScheduler",
            LibraryKind::TestHttpRequestMessage => "TestHttpRequestMessage",
            LibraryKind::TestHttpResponseMessage => "TestHttpResponseMessage",
            LibraryKind::TextBuilder => "TextBuilder",
            LibraryKind::Version => "Version",
            LibraryKind::WebServiceActionContext => "WebServiceActionContext",
            LibraryKind::XmlAttribute => "XmlAttribute",
            LibraryKind::XmlAttributeCollection => "XmlAttributeCollection",
            LibraryKind::XmlCData => "XmlCData",
            LibraryKind::XmlComment => "XmlComment",
            LibraryKind::XmlDeclaration => "XmlDeclaration",
            LibraryKind::XmlDocument => "XmlDocument",
            LibraryKind::XmlDocumentType => "XmlDocumentType",
            LibraryKind::XmlElement => "XmlElement",
            LibraryKind::XmlNamespaceManager => "XmlNamespaceManager",
            LibraryKind::XmlNameTable => "XmlNameTable",
            LibraryKind::XmlNode => "XmlNode",
            LibraryKind::XmlNodeList => "XmlNodeList",
            LibraryKind::XmlProcessingInstruction => "XmlProcessingInstruction",
            LibraryKind::XmlText => "XmlText",
            LibraryKind::Other(n) => n,
        }
    }
}

/// Debug prints the quoted name, as the `String` it replaced did (the `compile_dump` comparison relies on it).
impl fmt::Debug for LibraryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.name(), f)
    }
}

/// Serialized as the name, which is what the `String` it replaced wrote (the compile cache and the dumps keep their bytes).
impl serde::Serialize for LibraryKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> serde::Deserialize<'de> for LibraryKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<LibraryKind, D::Error> {
        let name = String::deserialize(d)?;
        Ok(LibraryKind::from_name(&name))
    }
}

impl Deref for LibraryKind {
    type Target = str;
    fn deref(&self) -> &str {
        self.name()
    }
}

impl fmt::Display for LibraryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl From<&str> for LibraryKind {
    fn from(n: &str) -> LibraryKind {
        LibraryKind::from_name(n)
    }
}

impl From<String> for LibraryKind {
    fn from(n: String) -> LibraryKind {
        LibraryKind::from_name(&n)
    }
}

impl From<&String> for LibraryKind {
    fn from(n: &String) -> LibraryKind {
        LibraryKind::from_name(n)
    }
}

impl PartialEq<str> for LibraryKind {
    fn eq(&self, other: &str) -> bool {
        self.name() == other
    }
}

impl PartialEq<&str> for LibraryKind {
    fn eq(&self, other: &&str) -> bool {
        self.name() == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_case_does_not_matter() {
        for k in [
            LibraryKind::JsonObject,
            LibraryKind::Blob,
            LibraryKind::XmlNodeList,
            LibraryKind::SecretText,
        ] {
            assert_eq!(LibraryKind::from_name(k.name()), k);
            assert_eq!(LibraryKind::from_name(&k.name().to_ascii_uppercase()), k);
        }
        assert_eq!(
            LibraryKind::from_name("Nothing Like It"),
            LibraryKind::Other("Nothing Like It".into())
        );
        assert_eq!(LibraryKind::from_name("Nothing Like It").name(), "Nothing Like It");
    }
}
