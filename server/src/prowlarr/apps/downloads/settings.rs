// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Download/Clients/*/*Settings.cs and the enums they list:
// QBittorrentPriority.cs, QBittorrentState.cs, QBittorrentContentLayout.cs, TransmissionPriority.cs,
// DelugePriority.cs, RTorrentPriority.cs, UTorrentPriority.cs, UTorrentState.cs, Flood/Types/AdditionalTags.cs,
// FreeboxDownloadPriority.cs, SabnzbdPriority.cs, NzbgetPriority.cs, NzbVortexPriority.cs;
// src/NzbDrone.Core/Validation/RuleBuilderExtensions.cs, Paths/PathValidator.cs), GPL-3.0.

//! Every download client's settings as Prowlarr declares them (labels, help texts, defaults, choices), and the
//! checks Prowlarr's settings validators run on them, with Prowlarr's messages.

use super::super::provider::{Choice, FieldSpec, FieldType, Initial, Options, Privacy, ProviderFailure, ValueKind};
use crate::prowlarr::validation;
use serde_json::{json, Map, Value};

/// `Aria2Settings`.
pub const ARIA2_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec {
        order: 1,
        name: "port",
        label: "Port",
        kind: ValueKind::Int,
        field_type: FieldType::Number,
        default: Initial::Int(6800),
        ..FieldSpec::BASE
    },
    FieldSpec { order: 2, name: "rpcPath", label: "XmlRpcPath", default: Initial::Text("/rpc"), ..FieldSpec::BASE },
    FieldSpec {
        order: 3,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "secretToken",
        label: "SecretToken",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        default: Initial::Text("MySecretToken"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "directory",
        label: "Directory",
        help_text: Some("DownloadClientAriaSettingsDirectoryHelpText"),
        ..FieldSpec::BASE
    },
];

/// `TorrentBlackholeSettings`.
pub const TORRENT_BLACKHOLE_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "torrentFolder",
        label: "TorrentBlackholeTorrentFolder",
        field_type: FieldType::Path,
        help_text: Some("BlackholeFolderHelpText"),
        help_tokens: &[("extension", ".torrent")],
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "saveMagnetFiles",
        label: "TorrentBlackholeSaveMagnetFiles",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("TorrentBlackholeSaveMagnetFilesHelpText"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "magnetFileExtension",
        label: "TorrentBlackholeSaveMagnetFilesExtension",
        help_text: Some("TorrentBlackholeSaveMagnetFilesExtensionHelpText"),
        default: Initial::Text(".magnet"),
        ..FieldSpec::BASE
    },
];

/// `UsenetBlackholeSettings`.
pub const USENET_BLACKHOLE_SETTINGS: &[FieldSpec] = &[FieldSpec {
    order: 0,
    name: "nzbFolder",
    label: "UsenetBlackholeNzbFolder",
    field_type: FieldType::Path,
    help_text: Some("BlackholeFolderHelpText"),
    help_tokens: &[("extension", ".nzb")],
    ..FieldSpec::BASE
}];

/// `DelugeSettings`.
pub const DELUGE_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(8112), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "Deluge")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientDelugeSettingsUrlBaseHelpText"),
        advanced: true,
        help_tokens: &[("url", "http://[host]:[port]/[urlBase]/json")],
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        default: Initial::Text("deluge"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        default: Initial::Text("prowlarr"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(DELUGE_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "addPaused",
        label: "DownloadClientSettingsAddPaused",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
];

/// `DownloadStationSettings`.
pub const DOWNLOAD_STATION_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("127.0.0.1"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(5000), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "Download Station")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec { order: 3, name: "username", label: "Username", privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 4,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "tvDirectory",
        label: "Directory",
        help_text: Some("DownloadClientDownloadStationSettingsDirectoryHelpText"),
        ..FieldSpec::BASE
    },
];

/// `FloodSettings`.
pub const FLOOD_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(3000), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "Flood")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientFloodSettingsUrlBaseHelpText"),
        help_tokens: &[("url", "[protocol]://[host]:[port]/[urlBase]/api")],
        ..FieldSpec::BASE
    },
    FieldSpec { order: 4, name: "username", label: "Username", privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 5,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "destination",
        label: "Destination",
        help_text: Some("DownloadClientSettingsDestinationHelpText"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "tags",
        label: "Tags",
        kind: ValueKind::TextList,
        field_type: FieldType::Tag,
        help_text: Some("DownloadClientFloodSettingsTagsHelpText"),
        default: Initial::Texts(&["prowlarr"]),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "additionalTags",
        label: "DownloadClientFloodSettingsAdditionalTags",
        kind: ValueKind::IntList,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientFloodSettingsAdditionalTagsHelpText"),
        advanced: true,
        options: Options::Labeled(ADDITIONAL_TAGS),
        default: Initial::Ints(&[]),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 9,
        name: "addPaused",
        label: "DownloadClientSettingsAddPaused",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
];

/// `FreeboxDownloadSettings`.
pub const FREEBOX_DOWNLOAD_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "host",
        label: "Host",
        help_text: Some("DownloadClientFreeboxSettingsHostHelpText"),
        help_tokens: &[("url", "mafreebox.freebox.fr")],
        default: Initial::Text("mafreebox.freebox.fr"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "port",
        label: "Port",
        kind: ValueKind::Int,
        help_text: Some("DownloadClientFreeboxSettingsPortHelpText"),
        default: Initial::Int(443),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "Freebox API")],
        default: Initial::Bool(true),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "apiUrl",
        label: "DownloadClientFreeboxSettingsApiUrl",
        help_text: Some("DownloadClientFreeboxSettingsApiUrlHelpText"),
        advanced: true,
        help_tokens: &[("url", "http://[host]:[port]/[api_base_url]/[api_version]/"), ("defaultApiUrl", "/api/v1/")],
        default: Initial::Text("/api/v1/"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "appId",
        label: "DownloadClientFreeboxSettingsAppId",
        help_text: Some("DownloadClientFreeboxSettingsAppIdHelpText"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "appToken",
        label: "DownloadClientFreeboxSettingsAppToken",
        field_type: FieldType::Password,
        help_text: Some("DownloadClientFreeboxSettingsAppTokenHelpText"),
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "destinationDirectory",
        label: "Destination",
        help_text: Some("DownloadClientSettingsDestinationHelpText"),
        advanced: true,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(FREEBOX_DOWNLOAD_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 9,
        name: "addPaused",
        label: "DownloadClientSettingsAddPaused",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
];

/// `HadoukenSettings`.
pub const HADOUKEN_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(7070), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "Hadouken")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientSettingsUrlBaseHelpText"),
        advanced: true,
        help_tokens: &[("clientName", "Hadouken"), ("url", "http://[host]:[port]/[urlBase]/api")],
        ..FieldSpec::BASE
    },
    FieldSpec { order: 4, name: "username", label: "Username", privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 5,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        default: Initial::Text("prowlarr"),
        ..FieldSpec::BASE
    },
];

/// `NzbVortexSettings`.
pub const NZB_VORTEX_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(4321), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientSettingsUrlBaseHelpText"),
        advanced: true,
        help_tokens: &[("clientName", "NZBVortex"), ("url", "http://[host]:[port]/[urlBase]/api")],
        ..FieldSpec::BASE
    },
    FieldSpec { order: 3, name: "apiKey", label: "ApiKey", privacy: Privacy::ApiKey, ..FieldSpec::BASE },
    FieldSpec {
        order: 4,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        default: Initial::Text("Prowlarr"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(NZB_VORTEX_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
];

/// `NzbgetSettings`.
pub const NZBGET_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(6789), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "NZBGet")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientSettingsUrlBaseHelpText"),
        advanced: true,
        help_tokens: &[("clientName", "NZBGet"), ("url", "http://[host]:[port]/[urlBase]/jsonrpc")],
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "username",
        label: "Username",
        privacy: Privacy::UserName,
        default: Initial::Text("nzbget"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        default: Initial::Text("tegbzn6789"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        default: Initial::Text("Prowlarr"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(NZBGET_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "addPaused",
        label: "DownloadClientSettingsAddPaused",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientNzbgetSettingsAddPausedHelpText"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
];

/// `PneumaticSettings`.
pub const PNEUMATIC_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "nzbFolder",
        label: "DownloadClientPneumaticSettingsNzbFolder",
        field_type: FieldType::Path,
        help_text: Some("DownloadClientPneumaticSettingsNzbFolderHelpText"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "strmFolder",
        label: "DownloadClientPneumaticSettingsStrmFolder",
        field_type: FieldType::Path,
        help_text: Some("DownloadClientPneumaticSettingsStrmFolderHelpText"),
        ..FieldSpec::BASE
    },
];

/// `QBittorrentSettings`.
pub const Q_BITTORRENT_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(8080), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientQbittorrentSettingsUseSslHelpText"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientSettingsUrlBaseHelpText"),
        advanced: true,
        help_tokens: &[("clientName", "qBittorrent"), ("url", "http://[host]:[port]/[urlBase]/api")],
        ..FieldSpec::BASE
    },
    FieldSpec { order: 4, name: "apiKey", label: "ApiKey", privacy: Privacy::ApiKey, ..FieldSpec::BASE },
    FieldSpec { order: 5, name: "username", label: "Username", privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 6,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        default: Initial::Text("prowlarr"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(Q_BITTORRENT_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 9,
        name: "initialState",
        label: "DownloadClientSettingsInitialState",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientQbittorrentSettingsInitialStateHelpText"),
        options: Options::Labeled(Q_BITTORRENT_STATE),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 10,
        name: "sequentialOrder",
        label: "DownloadClientQbittorrentSettingsSequentialOrder",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientQbittorrentSettingsSequentialOrderHelpText"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 11,
        name: "firstAndLast",
        label: "DownloadClientQbittorrentSettingsFirstAndLastFirst",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientQbittorrentSettingsFirstAndLastFirstHelpText"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 12,
        name: "contentLayout",
        label: "DownloadClientQbittorrentSettingsContentLayout",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientQbittorrentSettingsContentLayoutHelpText"),
        options: Options::Labeled(Q_BITTORRENT_CONTENT_LAYOUT),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
];

/// `SabnzbdSettings`.
pub const SABNZBD_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(8080), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "Sabnzbd")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientSettingsUrlBaseHelpText"),
        advanced: true,
        help_tokens: &[("clientName", "Sabnzbd"), ("url", "http://[host]:[port]/[urlBase]/api")],
        ..FieldSpec::BASE
    },
    FieldSpec { order: 4, name: "apiKey", label: "ApiKey", privacy: Privacy::ApiKey, ..FieldSpec::BASE },
    FieldSpec { order: 5, name: "username", label: "Username", privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 6,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        default: Initial::Text("prowlarr"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(SABNZBD_PRIORITY),
        default: Initial::Int(-100),
        ..FieldSpec::BASE
    },
];

/// `TransmissionSettings`.
pub const TRANSMISSION_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(9091), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "Transmission")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientTransmissionSettingsUrlBaseHelpText"),
        advanced: true,
        help_tokens: &[("clientName", "Transmission"), ("url", "http://[host]:[port]/[urlBase]/rpc"), ("defaultUrl", "/transmission/")],
        default: Initial::Text("/transmission/"),
        ..FieldSpec::BASE
    },
    FieldSpec { order: 4, name: "username", label: "Username", privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 5,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategorySubFolderHelpText"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "directory",
        label: "Directory",
        help_text: Some("DownloadClientTransmissionSettingsDirectoryHelpText"),
        advanced: true,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(TRANSMISSION_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 9,
        name: "addPaused",
        label: "DownloadClientSettingsAddPaused",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
];

/// `RTorrentSettings`.
pub const R_TORRENT_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(8080), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "rTorrent")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "DownloadClientRTorrentSettingsUrlPath",
        help_text: Some("DownloadClientRTorrentSettingsUrlPathHelpText"),
        help_tokens: &[("url", "http(s)://[host]:[port]/[urlPath]"), ("url2", "/plugins/rpc/rpc.php")],
        default: Initial::Text("RPC2"),
        ..FieldSpec::BASE
    },
    FieldSpec { order: 4, name: "username", label: "Username", privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 5,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        default: Initial::Text("prowlarr"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "directory",
        label: "Directory",
        help_text: Some("DownloadClientRTorrentSettingsDirectoryHelpText"),
        advanced: true,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(R_TORRENT_PRIORITY),
        default: Initial::Int(2),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 9,
        name: "addStopped",
        label: "DownloadClientRTorrentSettingsAddStopped",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientRTorrentSettingsAddStoppedHelpText"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
];

/// `UTorrentSettings`.
pub const U_TORRENT_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", default: Initial::Text("localhost"), ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(8080), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "UseSsl",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("DownloadClientSettingsUseSslHelpText"),
        help_tokens: &[("clientName", "uTorrent")],
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "urlBase",
        label: "UrlBase",
        help_text: Some("DownloadClientSettingsUrlBaseHelpText"),
        advanced: true,
        help_tokens: &[("clientName", "uTorrent"), ("url", "http://[host]:[port]/[urlBase]/api")],
        ..FieldSpec::BASE
    },
    FieldSpec { order: 4, name: "username", label: "Username", privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 5,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "category",
        label: "DefaultCategory",
        help_text: Some("DownloadClientSettingsDefaultCategoryHelpText"),
        default: Initial::Text("prowlarr"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsPriorityItemHelpText"),
        options: Options::Labeled(U_TORRENT_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "intialState",
        label: "DownloadClientSettingsInitialState",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("DownloadClientSettingsInitialStateHelpText"),
        options: Options::Labeled(U_TORRENT_STATE),
        help_tokens: &[("clientName", "uTorrent")],
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
];

/// `AdditionalTags`: (value, name, order, hint).
pub const ADDITIONAL_TAGS: &[Choice] = &[
    (0, "Collection", 7, "Big Buck Bunny Series"),
    (1, "Quality", 10, "Bluray-2160p"),
    (2, "Languages", 13, "English"),
    (3, "ReleaseGroup", 16, "Example-Raws"),
    (4, "Year", 19, "2020"),
    (5, "Indexer", 22, "Torznab"),
    (6, "Studio", 25, "C-SPAN"),
];

/// `DelugePriority`: (value, name, order, hint).
pub const DELUGE_PRIORITY: &[Choice] = &[(0, "Last", 0, "(0)"), (1, "First", 1, "(1)")];

/// `FreeboxDownloadPriority`: (value, name, order, hint).
pub const FREEBOX_DOWNLOAD_PRIORITY: &[Choice] = &[(0, "Last", 0, "(0)"), (1, "First", 1, "(1)")];

/// `NzbVortexPriority`: (value, name, order, hint).
pub const NZB_VORTEX_PRIORITY: &[Choice] = &[(-1, "Low", -1, "(-1)"), (0, "Normal", 0, "(0)"), (1, "High", 1, "(1)")];

/// `NzbgetPriority`: (value, name, order, hint).
pub const NZBGET_PRIORITY: &[Choice] = &[
    (-100, "VeryLow", -100, "(-100)"),
    (-50, "Low", -50, "(-50)"),
    (0, "Normal", 0, "(0)"),
    (50, "High", 50, "(50)"),
    (100, "VeryHigh", 100, "(100)"),
    (900, "Force", 900, "(900)"),
];

/// `QBittorrentContentLayout`: (value, name, order, hint).
pub const Q_BITTORRENT_CONTENT_LAYOUT: &[Choice] = &[(0, "Default", 0, "(0)"), (1, "Original", 1, "(1)"), (2, "Subfolder", 2, "(2)")];

/// `QBittorrentPriority`: (value, name, order, hint).
pub const Q_BITTORRENT_PRIORITY: &[Choice] = &[(0, "Last", 0, "(0)"), (1, "First", 1, "(1)")];

/// `QBittorrentState`: (value, name, order, hint).
pub const Q_BITTORRENT_STATE: &[Choice] = &[(0, "Started", 7, "(0)"), (1, "Force Started", 10, "(1)"), (2, "Stopped", 13, "(2)")];

/// `RTorrentPriority`: (value, name, order, hint).
pub const R_TORRENT_PRIORITY: &[Choice] =
    &[(0, "DoNotDownload", 0, "(0)"), (1, "Low", 1, "(1)"), (2, "Normal", 2, "(2)"), (3, "High", 3, "(3)")];

/// `SabnzbdPriority`: (value, name, order, hint).
pub const SABNZBD_PRIORITY: &[Choice] = &[
    (-100, "Default", -100, "(-100)"),
    (-2, "Paused", -2, "(-2)"),
    (-1, "Low", -1, "(-1)"),
    (0, "Normal", 0, "(0)"),
    (1, "High", 1, "(1)"),
    (2, "Force", 2, "(2)"),
];

/// `TransmissionPriority`: (value, name, order, hint).
pub const TRANSMISSION_PRIORITY: &[Choice] = &[(0, "Last", 0, "(0)"), (1, "First", 1, "(1)")];

/// `UTorrentPriority`: (value, name, order, hint).
pub const U_TORRENT_PRIORITY: &[Choice] = &[(0, "Last", 0, "(0)"), (1, "First", 1, "(1)")];

/// `UTorrentState`: (value, name, order, hint).
pub const U_TORRENT_STATE: &[Choice] =
    &[(0, "Start", 0, "(0)"), (1, "ForceStart", 1, "(1)"), (2, "Pause", 2, "(2)"), (3, "Stop", 3, "(3)")];

/// Runs a settings validator's rules on one provider's settings, collecting the failures in order.
pub struct Rules<'a> {
    /// The settings being checked.
    s: &'a Map<String, Value>,
    /// What failed so far.
    pub out: Vec<ProviderFailure>,
}

impl<'a> Rules<'a> {
    /// The rules on `s`, none run yet.
    pub fn on(s: &'a Map<String, Value>) -> Rules<'a> {
        Rules { s, out: Vec::new() }
    }

    /// A setting (`null` when it is not set).
    fn value(&self, field: &str) -> Value {
        self.s.get(field).cloned().unwrap_or(Value::Null)
    }

    /// A text setting, `None` when it is not set.
    pub fn text(&self, field: &str) -> Option<String> {
        self.s.get(field).and_then(Value::as_str).map(str::to_string)
    }

    /// Whether a text setting has something other than spaces in it (`IsNotNullOrWhiteSpace`).
    pub fn set(&self, field: &str) -> bool {
        self.text(field).is_some_and(|t| !t.trim().is_empty())
    }

    /// Adds a failure of `property` saying `message`.
    fn fail(&mut self, field: &str, property: &str, message: impl Into<String>) {
        let v = self.value(field);
        self.out.push(ProviderFailure::new(property, message).with_value(v));
    }

    /// `ValidHost`: not empty, and a host name or an IP address without `http://`.
    pub fn host(&mut self) -> &mut Self {
        let host = self.text("host");
        if host.as_deref().is_none_or(|h| h.trim().is_empty()) {
            self.fail("host", "Host", "'Host' must not be empty.");
        }
        let re = regex::Regex::new("(?i)^[-_a-z0-9.]+$").expect("a fixed pattern");
        if !host.as_deref().is_some_and(|h| re.is_match(h) || validation::is_valid_ip_address(h)) {
            self.fail("host", "Host", "must be valid Host without http://");
        }
        self
    }

    /// `InclusiveBetween(1, 65535)` on the port.
    pub fn port(&mut self) -> &mut Self {
        let port = self.s.get("port").and_then(Value::as_i64).unwrap_or(0);
        if let Some(f) = validation::between("Port", port, 1, 65535) {
            self.out.push(ProviderFailure::new("Port", f.message).with_value(json!(port)));
        }
        self
    }

    /// `ValidUrlBase` on a setting (only when it is set, if `when_set`).
    pub fn url_base(&mut self, field: &str, property: &str, when_set: bool) -> &mut Self {
        if when_set && !self.set(field) {
            return self;
        }
        if self.text(field).is_some_and(|t| !validation::is_valid_url_base(&t)) {
            self.fail(field, property, "Must be a valid URL path (ie: '/sonarr')");
        }
        self
    }

    /// `Matches(pattern)` with Prowlarr's message (a setting that is not set passes).
    pub fn matches(&mut self, field: &str, property: &str, pattern: &str, message: &str) -> &mut Self {
        let re = regex::Regex::new(pattern).expect("a fixed pattern");
        if self.text(field).is_some_and(|t| !re.is_match(&t)) {
            self.fail(field, property, message);
        }
        self
    }

    /// `Empty()` with Prowlarr's message, when `when`.
    pub fn empty_when(&mut self, field: &str, property: &str, when: bool, message: &str) -> &mut Self {
        if when && self.set(field) {
            self.fail(field, property, message);
        }
        self
    }

    /// `NotEmpty()`, with Prowlarr's message (or FluentValidation's), as a warning when `warning`, when `when`.
    pub fn not_empty(&mut self, field: &str, property: &str, message: Option<&str>, warning: bool, when: bool) -> &mut Self {
        if when && !self.set(field) {
            let msg = message.map(str::to_string).unwrap_or_else(|| format!("'{}' must not be empty.", validation::display_name(property)));
            let v = self.value(field);
            let mut f = ProviderFailure::new(property, msg).with_value(v);
            f.is_warning = warning;
            self.out.push(f);
        }
        self
    }

    /// `IsValidPath`: an absolute path (`Invalid Path: '<path>'`), when `when`.
    pub fn path(&mut self, field: &str, property: &str, when: bool) -> &mut Self {
        if !when {
            return self;
        }
        match self.text(field) {
            Some(p) if validation::is_valid_path(&p) => {}
            Some(p) => self.fail(field, property, format!("Invalid Path: '{p}'")),
            None => self.fail(field, property, "Invalid Path: '{path}'"),
        }
        self
    }

    /// The failures.
    pub fn done(&mut self) -> Vec<ProviderFailure> {
        std::mem::take(&mut self.out)
    }
}

/// Transmission's and Download Station's category rule.
const LETTERS_AND_DASH: &str = r"(?i)^\.?[-a-z]*$";

/// `QBittorrentSettingsValidator`.
pub fn qbittorrent(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    let key = r.set("apiKey");
    r.host().port().url_base("urlBase", "UrlBase", true);
    r.empty_when("username", "Username", key, "Username must be empty when using API Key.");
    r.empty_when("password", "Password", key, "Password must be empty when using API Key.");
    r.matches("category", "Category", r"^([^\\/](/?[^\\/])*)?$", r"Can not contain '\', '//', or start/end with '/'");
    r.done()
}

/// `TransmissionSettingsValidator` (Transmission and Vuze).
pub fn transmission(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    let dir = r.set("directory");
    r.host().port().url_base("urlBase", "UrlBase", false);
    r.matches("category", "Category", LETTERS_AND_DASH, "Allowed characters a-z and -");
    r.empty_when("category", "Category", dir, "Cannot use Category and Directory");
    r.done()
}

/// `DelugeSettingsValidator`.
pub fn deluge(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.host().port().matches("category", "Category", "^[-a-z0-9]*$", "Allowed characters a-z, 0-9 and -");
    r.done()
}

/// `RTorrentSettingsValidator`.
pub fn rtorrent(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.host().port().not_empty("category", "Category", Some("A category is recommended"), true, true);
    r.done()
}

/// `UTorrentSettingsValidator`.
pub fn utorrent(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.host().port().url_base("urlBase", "UrlBase", true).not_empty("category", "Category", None, false, true);
    r.done()
}

/// The validators that check only the host and port (`FloodSettingsValidator`).
pub fn host_and_port(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.host().port();
    r.done()
}

/// `Aria2SettingsValidator`: only the host.
pub fn aria2(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.host();
    r.done()
}

/// `HadoukenSettingsValidator`.
pub fn hadouken(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.host().port().url_base("urlBase", "UrlBase", true);
    r.not_empty("username", "Username", Some("Username must not be empty."), false, true);
    r.not_empty("password", "Password", Some("Password must not be empty."), false, true);
    r.done()
}

/// `DownloadStationSettingsValidator`.
pub fn download_station(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    let dir = r.set("tvDirectory");
    r.host().port();
    if dir {
        r.matches("tvDirectory", "TvDirectory", "^[^/].*$", "Cannot start with /");
    }
    r.matches("category", "Category", LETTERS_AND_DASH, "Allowed characters a-z and -");
    r.empty_when("category", "Category", dir, "Cannot use Category and Directory");
    r.done()
}

/// `FreeboxDownloadSettingsValidator`.
pub fn freebox(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    let (cat, dest) = (r.set("category"), r.set("destinationDirectory"));
    r.host().port();
    r.not_empty("apiUrl", "ApiUrl", Some("'API URL' must not be empty."), false, true);
    r.url_base("apiUrl", "ApiUrl", false);
    r.not_empty("appId", "AppId", Some("'App ID' must not be empty."), false, true);
    r.not_empty("appToken", "AppToken", Some("'App Token' must not be empty."), false, true);
    r.matches("category", "Category", LETTERS_AND_DASH, "Allowed characters a-z and -");
    r.path("destinationDirectory", "DestinationDirectory", dest);
    let both = "Cannot use 'Category' and 'Destination Directory' at the same time.";
    r.empty_when("destinationDirectory", "DestinationDirectory", cat, both);
    r.empty_when("category", "Category", dest, both);
    r.done()
}

/// `TorrentBlackholeSettingsValidator`.
pub fn torrent_blackhole(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.path("torrentFolder", "TorrentFolder", true).not_empty("magnetFileExtension", "MagnetFileExtension", None, false, true);
    r.done()
}

/// `UsenetBlackholeSettingsValidator`.
pub fn usenet_blackhole(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.path("nzbFolder", "NzbFolder", true);
    r.done()
}

/// `PneumaticSettingsValidator`.
pub fn pneumatic(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.path("nzbFolder", "NzbFolder", true).path("strmFolder", "StrmFolder", true);
    r.done()
}

/// `SabnzbdSettingsValidator`.
pub fn sabnzbd(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    let (key, user) = (r.set("apiKey"), r.set("username"));
    r.host().port().url_base("urlBase", "UrlBase", true);
    r.not_empty("apiKey", "ApiKey", Some("API Key is required when username/password are not configured"), false, !user);
    r.not_empty("username", "Username", Some("Username is required when API key is not configured"), false, !key);
    r.not_empty("password", "Password", Some("Password is required when API key is not configured"), false, !key);
    r.not_empty("category", "Category", Some("A category is recommended"), true, true);
    r.done()
}

/// `NzbgetSettingsValidator`.
pub fn nzbget(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    let (user, pass) = (r.set("username"), r.set("password"));
    r.host().port().url_base("urlBase", "UrlBase", true);
    r.not_empty("username", "Username", None, false, pass);
    r.not_empty("password", "Password", None, false, user);
    r.not_empty("category", "Category", Some("A category is recommended"), true, true);
    r.done()
}

/// `NzbVortexSettingsValidator`.
pub fn nzbvortex(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.host().port().url_base("urlBase", "UrlBase", true);
    r.not_empty("apiKey", "ApiKey", Some("API Key is required"), false, true);
    r.not_empty("category", "Category", Some("A category is recommended"), true, true);
    r.done()
}
