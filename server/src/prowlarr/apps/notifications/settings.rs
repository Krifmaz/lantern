// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Notifications/*/*Settings.cs and the enums they list:
// AppriseNotificationType.cs, DiscordFieldType.cs, EmailEncryptionType (EmailSettings.cs), GotifyPriority.cs,
// JoinPriority.cs, NtfyPriority.cs, ProwlPriority.cs, PushoverPriority.cs, WebhookMethod.cs;
// src/NzbDrone.Core/Validation/Paths/SystemFolderValidator.cs, src/NzbDrone.Common/Disk/SystemFolders.cs), GPL-3.0.

//! Every notification's settings as Prowlarr declares them, and the checks its settings validators run on them.

use super::super::downloads::settings::Rules;
use super::super::provider::{
    check_not_empty, check_url, Choice, FieldSpec, FieldType, Initial, Options, Privacy, ProviderFailure, ValueKind,
};
use crate::prowlarr::validation;
use serde_json::{json, Map, Value};

/// `AppriseSettings`.
pub const APPRISE_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 1, name: "serverUrl", label: "Apprise Server URL", field_type: FieldType::Url, help_text: Some("Apprise server URL, including http(s):// and port if needed"), help_link: Some("https://github.com/caronc/apprise-api"), placeholder: Some("http://localhost:8000"), ..FieldSpec::BASE },
    FieldSpec { order: 2, name: "configurationKey", label: "Apprise Configuration Key", help_text: Some("Configuration Key for the Persistent Storage Solution. Leave empty if Stateless URLs is used."), help_link: Some("https://github.com/caronc/apprise-api#persistent-storage-solution"), ..FieldSpec::BASE },
    FieldSpec { order: 3, name: "statelessUrls", label: "Apprise Stateless URLs", help_text: Some("One or more URLs separated by commas identifying where the notification should be sent to. Leave empty if Persistent Storage is used."), help_link: Some("https://github.com/caronc/apprise#productivity-based-notifications"), ..FieldSpec::BASE },
    FieldSpec { order: 4, name: "notificationType", label: "Apprise Notification Type", kind: ValueKind::Int, field_type: FieldType::Select, options: Options::Labeled(APPRISE_NOTIFICATION_TYPE), default: Initial::Int(0), ..FieldSpec::BASE },
    FieldSpec { order: 5, name: "tags", label: "Apprise Tags", kind: ValueKind::TextList, field_type: FieldType::Tag, help_text: Some("Optionally notify only those tagged accordingly."), default: Initial::Texts(&[]), ..FieldSpec::BASE },
    FieldSpec { order: 6, name: "authUsername", label: "Username", help_text: Some("HTTP Basic Auth Username"), privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec { order: 7, name: "authPassword", label: "Password", field_type: FieldType::Password, help_text: Some("HTTP Basic Auth Password"), privacy: Privacy::Password, ..FieldSpec::BASE },
];

/// `CustomScriptSettings`.
pub const CUSTOM_SCRIPT_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "path", label: "Path", field_type: FieldType::FilePath, ..FieldSpec::BASE },
    FieldSpec {
        order: 1,
        name: "arguments",
        label: "Arguments",
        help_text: Some("Arguments to pass to the script"),
        hidden: Some("hiddenIfNotSet"),
        ..FieldSpec::BASE
    },
];

/// `DiscordSettings`.
pub const DISCORD_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "webHookUrl", label: "Webhook URL", help_text: Some("Discord channel webhook url"), ..FieldSpec::BASE },
    FieldSpec {
        order: 1,
        name: "username",
        label: "Username",
        help_text: Some("The username to post as, defaults to Discord webhook default"),
        privacy: Privacy::UserName,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "avatar",
        label: "Avatar",
        help_text: Some("Change the avatar that is used for messages from this integration"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "author",
        label: "Host",
        help_text: Some("Override the Host that shows for this notification, Blank is machine name"),
        advanced: true,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "grabFields",
        label: "On Grab Fields",
        kind: ValueKind::IntList,
        field_type: FieldType::Select,
        help_text: Some("Change the fields that are passed in for this 'on grab' notification"),
        advanced: true,
        options: Options::Labeled(DISCORD_GRAB_FIELD_TYPE),
        default: Initial::Ints(&[0, 1, 2, 3, 4, 5]),
        ..FieldSpec::BASE
    },
];

/// `EmailSettings`.
pub const EMAIL_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "server",
        label: "Server",
        help_text: Some("Hostname or IP of Email server"),
        placeholder: Some("smtp.gmail.com"),
        ..FieldSpec::BASE
    },
    FieldSpec { order: 1, name: "port", label: "Port", kind: ValueKind::Int, default: Initial::Int(587), ..FieldSpec::BASE },
    FieldSpec {
        order: 2,
        name: "useEncryption",
        label: "NotificationsEmailSettingsUseEncryption",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("NotificationsEmailSettingsUseEncryptionHelpText"),
        options: Options::Labeled(EMAIL_ENCRYPTION_TYPE),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec { order: 3, name: "username", label: "Username", help_text: Some("Username"), privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 4,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        help_text: Some("Password"),
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec { order: 5, name: "from", label: "From Address", placeholder: Some("example@email.com"), ..FieldSpec::BASE },
    FieldSpec {
        order: 6,
        name: "to",
        label: "Recipient Address(es)",
        kind: ValueKind::TextList,
        help_text: Some("Comma separated list of email recipients"),
        placeholder: Some("example@email.com,example1@email.com"),
        default: Initial::Texts(&[]),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "cc",
        label: "CC Address(es)",
        kind: ValueKind::TextList,
        help_text: Some("Comma separated list of email cc recipients"),
        placeholder: Some("example@email.com,example1@email.com"),
        advanced: true,
        default: Initial::Texts(&[]),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 8,
        name: "bcc",
        label: "BCC Address(es)",
        kind: ValueKind::TextList,
        help_text: Some("Comma separated list of email bcc recipients"),
        placeholder: Some("example@email.com,example1@email.com"),
        advanced: true,
        default: Initial::Texts(&[]),
        ..FieldSpec::BASE
    },
];

/// `GotifySettings`.
pub const GOTIFY_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "server",
        label: "Gotify Server",
        help_text: Some("Gotify server URL, including http(s):// and port if needed"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "appToken",
        label: "App Token",
        help_text: Some("The Application Token generated by Gotify"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("Priority of the notification"),
        options: Options::Labeled(GOTIFY_PRIORITY),
        default: Initial::Int(5),
        ..FieldSpec::BASE
    },
];

/// `JoinSettings`.
pub const JOIN_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "apiKey", label: "API Key", help_text: Some("The API Key from your Join account settings (click Join API button)."), help_link: Some("https://joinjoaomgcd.appspot.com/"), privacy: Privacy::ApiKey, ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "deviceIds", label: "Device IDs", help_text: Some("Deprecated, use Device Names instead. Comma separated list of Device IDs you'd like to send notifications to. If unset, all devices will receive notifications."), ..FieldSpec::BASE },
    FieldSpec { order: 2, name: "deviceNames", label: "Device Names", help_text: Some("Comma separated list of full or partial device names you'd like to send notifications to. If unset, all devices will receive notifications."), help_link: Some("https://joaoapps.com/join/api/"), ..FieldSpec::BASE },
    FieldSpec { order: 3, name: "priority", label: "Notification Priority", kind: ValueKind::Int, field_type: FieldType::Select, options: Options::Labeled(JOIN_PRIORITY), default: Initial::Int(0), ..FieldSpec::BASE },
];

/// `MailgunSettings`.
pub const MAILGUN_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "apiKey", label: "API Key", help_text: Some("The API key generated from MailGun"), ..FieldSpec::BASE },
    FieldSpec {
        order: 1,
        name: "useEuEndpoint",
        label: "Use EU Endpoint?",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("Use the EU MailGun endpoint"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec { order: 2, name: "from", label: "From Address", ..FieldSpec::BASE },
    FieldSpec { order: 3, name: "senderDomain", label: "Sender Domain", ..FieldSpec::BASE },
    FieldSpec {
        order: 4,
        name: "recipients",
        label: "Recipient Address(es)",
        kind: ValueKind::TextList,
        field_type: FieldType::Tag,
        placeholder: Some("example@email.com,example1@email.com"),
        ..FieldSpec::BASE
    },
];

/// `NotifiarrSettings`.
pub const NOTIFIARR_SETTINGS: &[FieldSpec] = &[FieldSpec {
    order: 0,
    name: "aPIKey",
    label: "API Key",
    help_text: Some("Your API key from your profile"),
    help_link: Some("https://notifiarr.com"),
    privacy: Privacy::ApiKey,
    ..FieldSpec::BASE
}];

/// `NtfySettings`.
pub const NTFY_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "serverUrl",
        label: "Server Url",
        field_type: FieldType::Url,
        help_text: Some("Leave blank to use public server (https://ntfy.sh)"),
        help_link: Some("https://ntfy.sh/docs/install/"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "accessToken",
        label: "Access Token",
        field_type: FieldType::Password,
        help_text: Some("Optional token-based authorization. Takes priority over username/password"),
        help_link: Some("https://docs.ntfy.sh/config/#access-tokens"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "userName",
        label: "User Name",
        help_text: Some("Optional Authorization"),
        privacy: Privacy::UserName,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        help_text: Some("Optional Password"),
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        options: Options::Labeled(NTFY_PRIORITY),
        default: Initial::Int(3),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "topics",
        label: "Topics",
        kind: ValueKind::TextList,
        field_type: FieldType::Tag,
        help_text: Some("List of Topics to send notifications to"),
        default: Initial::Texts(&[]),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "tags",
        label: "Ntfy Tags and Emojis",
        kind: ValueKind::TextList,
        field_type: FieldType::Tag,
        help_text: Some("Optional list of tags or emojis to use"),
        help_link: Some("https://ntfy.sh/docs/emojis/"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 7,
        name: "clickUrl",
        label: "Click URL",
        field_type: FieldType::Url,
        help_text: Some("Optional link when user clicks notification"),
        ..FieldSpec::BASE
    },
];

/// `ProwlSettings`.
pub const PROWL_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "apiKey",
        label: "API Key",
        help_link: Some("https://www.prowlapp.com/api_settings.php"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        options: Options::Labeled(PROWL_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
];

/// `PushBulletSettings`.
pub const PUSH_BULLET_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "apiKey", label: "Access Token", help_link: Some("https://www.pushbullet.com/#settings/account"), privacy: Privacy::ApiKey, ..FieldSpec::BASE },
    FieldSpec { order: 1, name: "deviceIds", label: "Device IDs", kind: ValueKind::TextList, field_type: FieldType::Device, help_text: Some("List of device IDs (leave blank to send to all devices)"), placeholder: Some("123456789,987654321"), default: Initial::Texts(&[]), ..FieldSpec::BASE },
    FieldSpec { order: 2, name: "channelTags", label: "Channel Tags", kind: ValueKind::TextList, field_type: FieldType::Tag, help_text: Some("List of Channel Tags to send notifications to"), placeholder: Some("Channel1234,Channel4321"), default: Initial::Texts(&[]), ..FieldSpec::BASE },
    FieldSpec { order: 3, name: "senderId", label: "Sender ID", help_text: Some("The device ID to send notifications from, use device_iden in the device's URL on pushbullet.com (leave blank to send from yourself)"), ..FieldSpec::BASE },
];

/// `PushcutSettings`.
pub const PUSHCUT_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "notificationName",
        label: "Notification name",
        help_text: Some("Notification name from Notifications tab of the Pushcut app."),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "apiKey",
        label: "API Key",
        help_text: Some("API Keys can be managed in the Account view of the Pushcut app."),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "timeSensitive",
        label: "Time sensitive",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("Check to mark the notification as \\\"Time-Sensitive\\\""),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
];

/// `PushoverSettings`.
pub const PUSHOVER_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "apiKey",
        label: "API Key",
        help_link: Some("https://pushover.net/apps/clone/sonarr"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "userKey",
        label: "User Key",
        help_link: Some("https://pushover.net/"),
        privacy: Privacy::UserName,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "devices",
        label: "Devices",
        kind: ValueKind::TextList,
        field_type: FieldType::Tag,
        help_text: Some("List of device names (leave blank to send to all devices)"),
        placeholder: Some("device1"),
        default: Initial::Texts(&[]),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "priority",
        label: "Priority",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        options: Options::Labeled(PUSHOVER_PRIORITY),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "retry",
        label: "Retry",
        kind: ValueKind::Int,
        help_text: Some("Interval to retry Emergency alerts, minimum 30 seconds"),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "expire",
        label: "Expire",
        kind: ValueKind::Int,
        help_text: Some("Maximum time to retry Emergency alerts, maximum 86400 seconds"),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "sound",
        label: "Sound",
        help_text: Some("Notification sound, leave blank to use the default"),
        help_link: Some("https://pushover.net/api#sounds"),
        ..FieldSpec::BASE
    },
];

/// `SendGridSettings`.
pub const SEND_GRID_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 1,
        name: "apiKey",
        label: "API Key",
        help_text: Some("The API Key generated by SendGrid"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec { order: 2, name: "from", label: "From Address", ..FieldSpec::BASE },
    FieldSpec {
        order: 3,
        name: "recipients",
        label: "Recipient Address(es)",
        kind: ValueKind::TextList,
        field_type: FieldType::Tag,
        placeholder: Some("example@email.com,example1@email.com"),
        default: Initial::Texts(&[]),
        ..FieldSpec::BASE
    },
];

/// `SignalSettings`.
pub const SIGNAL_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "host", label: "Host", help_text: Some("localhost"), ..FieldSpec::BASE },
    FieldSpec {
        order: 1,
        name: "port",
        label: "Port",
        kind: ValueKind::Int,
        help_text: Some("8080"),
        default: Initial::Int(0),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "useSsl",
        label: "Use SSL",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("Use a secure connection."),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "senderNumber",
        label: "Sender Number",
        help_text: Some("Phone number of the sender register in signal-api"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "receiverId",
        label: "Group ID / PhoneNumber",
        help_text: Some("GroupID / PhoneNumber of the receiver"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 5,
        name: "authUsername",
        label: "Login",
        help_text: Some("Username used to authenticate requests toward signal-api"),
        privacy: Privacy::UserName,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "authPassword",
        label: "Password",
        field_type: FieldType::Password,
        help_text: Some("Password used to authenticate requests toward signal-api"),
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
];

/// `SimplepushSettings`.
pub const SIMPLEPUSH_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "key",
        label: "Key",
        help_link: Some("https://simplepush.io/features"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "event",
        label: "Event",
        help_text: Some("Customize the behavior of push notifications"),
        help_link: Some("https://simplepush.io/features"),
        ..FieldSpec::BASE
    },
];

/// `SlackSettings`.
pub const SLACK_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "webHookUrl",
        label: "Webhook URL",
        field_type: FieldType::Url,
        help_text: Some("Slack channel webhook url"),
        help_link: Some("https://my.slack.com/services/new/incoming-webhook/"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "username",
        label: "Username",
        help_text: Some("Choose the username that this integration will post as"),
        privacy: Privacy::UserName,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "icon",
        label: "Icon",
        help_text: Some("Change the icon that is used for messages from this integration (Emoji or URL)"),
        help_link: Some("http://www.emoji-cheat-sheet.com/"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "channel",
        label: "Channel",
        help_text: Some("Overrides the default channel for the incoming webhook (#other-channel)"),
        ..FieldSpec::BASE
    },
];

/// `TelegramSettings`.
pub const TELEGRAM_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "botToken",
        label: "Bot Token",
        help_link: Some("https://core.telegram.org/bots"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "chatId",
        label: "Chat ID",
        help_text: Some("You must start a conversation with the bot or add it to your group to receive messages"),
        help_link: Some("http://stackoverflow.com/a/37396871/882971"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "topicId",
        label: "Topic ID",
        kind: ValueKind::OptInt,
        help_text: Some("Specify a Topic ID to send notifications to that topic. Leave blank to use the general topic (Supergroups only)"),
        help_link: Some("https://stackoverflow.com/a/75178418"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 3,
        name: "sendSilently",
        label: "Send Silently",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("Sends the message silently. Users will receive a notification with no sound"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 4,
        name: "includeAppNameInTitle",
        label: "NotificationsTelegramSettingsIncludeAppName",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("NotificationsTelegramSettingsIncludeAppNameHelpText"),
        default: Initial::Bool(false),
        ..FieldSpec::BASE
    },
];

/// `TwitterSettings`.
pub const TWITTER_SETTINGS: &[FieldSpec] = &[
    FieldSpec {
        order: 0,
        name: "consumerKey",
        label: "Consumer Key",
        help_text: Some("Consumer key from a Twitter application"),
        help_link: Some("https://wiki.servarr.com/useful-tools#twitter-connect"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 1,
        name: "consumerSecret",
        label: "Consumer Secret",
        help_text: Some("Consumer secret from a Twitter application"),
        help_link: Some("https://wiki.servarr.com/useful-tools#twitter-connect"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec { order: 2, name: "accessToken", label: "Access Token", advanced: true, privacy: Privacy::ApiKey, ..FieldSpec::BASE },
    FieldSpec {
        order: 3,
        name: "accessTokenSecret",
        label: "Access Token Secret",
        advanced: true,
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
    FieldSpec { order: 4, name: "mention", label: "Mention", help_text: Some("Mention this user in sent tweets"), ..FieldSpec::BASE },
    FieldSpec {
        order: 5,
        name: "directMessage",
        label: "Direct Message",
        kind: ValueKind::Bool,
        field_type: FieldType::Checkbox,
        help_text: Some("Send a direct message instead of a public message"),
        default: Initial::Bool(true),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 6,
        name: "authorizeNotification",
        label: "Connect to Twitter",
        field_type: FieldType::OAuth,
        default: Initial::Text("startOAuth"),
        ..FieldSpec::BASE
    },
];

/// `WebhookSettings`.
pub const WEBHOOK_SETTINGS: &[FieldSpec] = &[
    FieldSpec { order: 0, name: "url", label: "URL", field_type: FieldType::Url, ..FieldSpec::BASE },
    FieldSpec {
        order: 1,
        name: "method",
        label: "Method",
        kind: ValueKind::Int,
        field_type: FieldType::Select,
        help_text: Some("Which HTTP method to use submit to the Webservice"),
        options: Options::Labeled(WEBHOOK_METHOD),
        default: Initial::Int(1),
        ..FieldSpec::BASE
    },
    FieldSpec { order: 2, name: "username", label: "Username", help_text: Some("Username"), privacy: Privacy::UserName, ..FieldSpec::BASE },
    FieldSpec {
        order: 3,
        name: "password",
        label: "Password",
        field_type: FieldType::Password,
        help_text: Some("Password"),
        privacy: Privacy::Password,
        ..FieldSpec::BASE
    },
];

/// `AppriseNotificationType`: (value, name, order, hint).
pub const APPRISE_NOTIFICATION_TYPE: &[Choice] =
    &[(0, "Info", 0, "(0)"), (1, "Success", 1, "(1)"), (2, "Warning", 2, "(2)"), (3, "Failure", 3, "(3)")];

/// `DiscordGrabFieldType`: (value, name, order, hint).
pub const DISCORD_GRAB_FIELD_TYPE: &[Choice] = &[
    (0, "Release", 0, "(0)"),
    (1, "Indexer", 1, "(1)"),
    (2, "DownloadClient", 2, "(2)"),
    (3, "GrabTrigger", 3, "(3)"),
    (4, "Source", 4, "(4)"),
    (5, "Host", 5, "(5)"),
];

/// `EmailEncryptionType`: (value, name, order, hint).
pub const EMAIL_ENCRYPTION_TYPE: &[Choice] = &[(0, "Preferred", 0, "(0)"), (1, "Always", 1, "(1)"), (2, "Never", 2, "(2)")];

/// `GotifyPriority`: (value, name, order, hint).
pub const GOTIFY_PRIORITY: &[Choice] = &[(0, "Min", 0, "(0)"), (2, "Low", 2, "(2)"), (5, "Normal", 5, "(5)"), (8, "High", 8, "(8)")];

/// `JoinPriority`: (value, name, order, hint).
pub const JOIN_PRIORITY: &[Choice] =
    &[(-2, "Silent", -2, "(-2)"), (-1, "Quiet", -1, "(-1)"), (0, "Normal", 0, "(0)"), (1, "High", 1, "(1)"), (2, "Emergency", 2, "(2)")];

/// `NtfyPriority`: (value, name, order, hint).
pub const NTFY_PRIORITY: &[Choice] =
    &[(1, "Min", 1, "(1)"), (2, "Low", 2, "(2)"), (3, "Default", 3, "(3)"), (4, "High", 4, "(4)"), (5, "Max", 5, "(5)")];

/// `ProwlPriority`: (value, name, order, hint).
pub const PROWL_PRIORITY: &[Choice] =
    &[(-2, "VeryLow", -2, "(-2)"), (-1, "Low", -1, "(-1)"), (0, "Normal", 0, "(0)"), (1, "High", 1, "(1)"), (2, "Emergency", 2, "(2)")];

/// `PushoverPriority`: (value, name, order, hint).
pub const PUSHOVER_PRIORITY: &[Choice] =
    &[(-2, "Silent", -2, "(-2)"), (-1, "Quiet", -1, "(-1)"), (0, "Normal", 0, "(0)"), (1, "High", 1, "(1)"), (2, "Emergency", 2, "(2)")];

/// `WebhookMethod`: (value, name, order, hint).
pub const WEBHOOK_METHOD: &[Choice] = &[(1, "POST", 1, "(1)"), (2, "PUT", 2, "(2)")];

/// FluentValidation's `EmailAddress` (a text with an `@` that is neither first nor last).
fn is_email(s: &str) -> bool {
    let i = s.find('@');
    i.is_some_and(|i| i > 0 && i < s.len() - 1) && s.matches('@').count() == 1
}

/// A list setting's texts.
fn texts(s: &Map<String, Value>, field: &str) -> Vec<String> {
    s.get(field).and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

/// `RuleForEach(..).EmailAddress()`: each address in a list (`'To[0]' is not a valid email address.`).
fn each_email(r: &mut Rules, s: &Map<String, Value>, field: &str, property: &str) {
    for (i, t) in texts(s, field).iter().enumerate() {
        if !is_email(t) {
            r.out.push(
                ProviderFailure::new(&format!("{property}[{i}]"), format!("'{property}[{i}]' is not a valid email address."))
                    .with_value(json!(t)),
            );
        }
    }
}

/// `IsValidUrl` on a setting, when `when`.
fn url(r: &mut Rules, s: &Map<String, Value>, field: &str, property: &str, when: bool) {
    if when {
        if let Some(f) = check_url(s, field, property) {
            r.out.push(f);
        }
    }
}

/// A validator that only needs these settings not empty: (field, property).
fn required(s: &Map<String, Value>, fields: &[(&str, &str)]) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    for (f, p) in fields {
        if let Some(x) = check_not_empty(s, f, p) {
            r.out.push(x);
        }
    }
    r.done()
}

/// `AppriseSettingsValidator`.
pub fn apprise(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    let (key, urls) = (r.set("configurationKey"), r.set("statelessUrls"));
    url(&mut r, s, "serverUrl", "ServerUrl", true);
    let either = "Use either Configuration Key or Stateless URLs";
    r.not_empty("configurationKey", "ConfigurationKey", Some(either), false, !urls);
    r.matches("configurationKey", "ConfigurationKey", "^[a-z0-9-]*$", "Allowed characters a-z, 0-9 and -");
    r.not_empty("statelessUrls", "StatelessUrls", Some(either), false, !key);
    r.empty_when("statelessUrls", "StatelessUrls", key, either);
    if urls && !texts(s, "tags").is_empty() {
        r.out.push(
            ProviderFailure::new("Tags", "Stateless URLs do not support tags").with_value(s.get("tags").cloned().unwrap_or(Value::Null)),
        );
    }
    r.done()
}

/// The folders a script may not be in (`SystemFolders` on Linux).
const SYSTEM_FOLDERS: [&str; 6] = ["/bin", "/boot", "/lib", "/sbin", "/proc", "/usr/bin"];

/// `CustomScriptSettingsValidator`: a valid path outside the system folders, and no arguments.
pub fn custom_script(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.path("path", "Path", true);
    let path = r.text("path").unwrap_or_default();
    let p = path.trim_end_matches('/');
    for sys in SYSTEM_FOLDERS {
        if p == sys || p.starts_with(&format!("{sys}/")) {
            r.out.push(ProviderFailure::new("Path", format!("Must not be a descendant of '{sys}'")).with_value(json!(path)));
            break;
        }
    }
    r.empty_when("arguments", "Arguments", true, "Arguments are no longer supported for custom scripts");
    r.done()
}

/// `DiscordSettingsValidator` and `SlackSettingsValidator`: a valid webhook address.
pub fn webhook_url(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    url(&mut r, s, "webHookUrl", "WebHookUrl", true);
    r.done()
}

/// `EmailSettingsValidator`.
pub fn email(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.not_empty("server", "Server", None, false, true);
    let port = s.get("port").and_then(Value::as_i64).unwrap_or(0);
    if let Some(f) = validation::between("Port", port, 1, 65535) {
        r.out.push(ProviderFailure::new("Port", f.message).with_value(json!(port)));
    }
    r.not_empty("from", "From", None, false, true);
    each_email(&mut r, s, "to", "To");
    each_email(&mut r, s, "cc", "Cc");
    each_email(&mut r, s, "bcc", "Bcc");
    let (to, cc, bcc) = (!texts(s, "to").is_empty(), !texts(s, "cc").is_empty(), !texts(s, "bcc").is_empty());
    for (field, property, others) in [("to", "To", bcc || cc), ("cc", "Cc", to || bcc), ("bcc", "Bcc", to || cc)] {
        if !others {
            if let Some(f) = check_not_empty(s, field, property) {
                r.out.push(f);
            }
        }
    }
    r.done()
}

/// `GotifySettingsValidator`.
pub fn gotify(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    url(&mut r, s, "server", "Server", true);
    r.not_empty("appToken", "AppToken", None, false, true);
    r.done()
}

/// `JoinSettingsValidator`.
pub fn join(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.not_empty("apiKey", "ApiKey", None, false, true);
    r.empty_when("deviceIds", "DeviceIds", true, "Use Device Names instead");
    r.done()
}

/// `MailgunSettingsValidator`.
pub fn mailgun(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    required(s, &[("apiKey", "ApiKey"), ("from", "From"), ("recipients", "Recipients")])
}

/// `NotifiarrSettingsValidator`.
pub fn notifiarr(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    required(s, &[("aPIKey", "APIKey")])
}

/// Topics ntfy keeps for itself.
const INVALID_TOPICS: [&str; 8] = ["announcements", "app", "docs", "settings", "stats", "mytopic-rw", "mytopic-ro", "mytopic-wo"];

/// `NtfySettingsValidator`.
pub fn ntfy(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    if let Some(f) = check_not_empty(s, "topics", "Topics") {
        r.out.push(f);
    }
    let prio = s.get("priority").and_then(Value::as_i64).unwrap_or(0);
    if let Some(f) = validation::between("Priority", prio, 1, 5) {
        r.out.push(ProviderFailure::new("Priority", f.message).with_value(json!(prio)));
    }
    let server = r.set("serverUrl");
    url(&mut r, s, "serverUrl", "ServerUrl", server);
    let click = r.set("clickUrl");
    url(&mut r, s, "clickUrl", "ClickUrl", click);
    let (user, pass, token) = (r.set("userName"), r.set("password"), r.set("accessToken"));
    r.not_empty("userName", "UserName", None, false, pass && !token);
    r.not_empty("password", "Password", None, false, user && !token);
    let topic_re = regex::Regex::new("[a-zA-Z0-9_-]+").expect("a fixed pattern");
    for (i, t) in texts(s, "topics").iter().enumerate() {
        let property = format!("Topics[{i}]");
        if t.trim().is_empty() {
            r.out.push(ProviderFailure::new(&property, format!("'{property}' must not be empty.")).with_value(json!(t)));
        }
        if !topic_re.is_match(t) || INVALID_TOPICS.contains(&t.as_str()) {
            r.out.push(ProviderFailure::new(&property, "Invalid topic").with_value(json!(t)));
        }
    }
    r.done()
}

/// `ProwlSettingsValidator`, `PushBulletSettingsValidator`, `SimplepushSettingsValidator` and the like: a key.
pub fn api_key(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    required(s, &[("apiKey", "ApiKey")])
}

/// `PushcutSettingsValidator`.
pub fn pushcut(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    required(s, &[("notificationName", "NotificationName"), ("apiKey", "ApiKey")])
}

/// `PushoverSettingsValidator`: the keys, and with emergency priority (2) a retry of 30 seconds to a day.
pub fn pushover(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.out.extend(required(s, &[("apiKey", "ApiKey"), ("userKey", "UserKey")]));
    if s.get("priority").and_then(Value::as_i64) == Some(2) {
        let retry = s.get("retry").and_then(Value::as_i64).unwrap_or(0);
        if retry < 30 {
            r.out.push(ProviderFailure::new("Retry", format!("'Retry' must be greater than or equal to '30'.")).with_value(json!(retry)));
        }
        if retry > 86400 {
            r.out.push(ProviderFailure::new("Retry", "'Retry' must be less than or equal to '86400'.").with_value(json!(retry)));
        }
    }
    r.done()
}

/// `SendGridSettingsValidator`.
pub fn sendgrid(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.out.extend(required(s, &[("apiKey", "ApiKey"), ("from", "From")]));
    if r.text("from").is_some_and(|f| !f.trim().is_empty() && !is_email(&f)) {
        r.out.push(
            ProviderFailure::new("From", "'From' is not a valid email address.").with_value(s.get("from").cloned().unwrap_or(Value::Null)),
        );
    }
    r.out.extend(required(s, &[("recipients", "Recipients")]));
    each_email(&mut r, s, "recipients", "Recipients");
    r.done()
}

/// `SignalSettingsValidator`.
pub fn signal(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.out.extend(required(s, &[("host", "Host")]));
    if s.get("port").and_then(Value::as_i64).unwrap_or(0) == 0 {
        r.out.push(ProviderFailure::new("Port", "'Port' must not be empty.").with_value(json!(0)));
    }
    r.out.extend(required(s, &[("senderNumber", "SenderNumber"), ("receiverId", "ReceiverId")]));
    r.done()
}

/// `SimplepushSettingsValidator`.
pub fn simplepush(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    required(s, &[("key", "Key")])
}

/// `TelegramSettingsValidator`.
pub fn telegram(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.out.extend(required(s, &[("botToken", "BotToken"), ("chatId", "ChatId")]));
    if s.get("topicId").and_then(Value::as_i64).is_some_and(|t| t <= 1) {
        r.out.push(ProviderFailure::new("TopicId", "Topic ID must be greater than 1 or empty").with_value(s["topicId"].clone()));
    }
    r.done()
}

/// `TwitterSettingsValidator`.
pub fn twitter(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    r.out.extend(required(
        s,
        &[
            ("consumerKey", "ConsumerKey"),
            ("consumerSecret", "ConsumerSecret"),
            ("accessToken", "AccessToken"),
            ("accessTokenSecret", "AccessTokenSecret"),
        ],
    ));
    let dm = s.get("directMessage").and_then(Value::as_bool).unwrap_or(false);
    r.not_empty("mention", "Mention", None, false, dm);
    if !dm {
        r.out.push(
            ProviderFailure::new("DirectMessage", "Using Direct Messaging is recommended, or use a private account.")
                .with_value(json!(dm))
                .warning(),
        );
    }
    let tokens = r.set("accessToken") && r.set("accessTokenSecret");
    r.empty_when("authorizeNotification", "AuthorizeNotification", !tokens, "Authenticate app.");
    r.done()
}

/// `WebhookSettingsValidator`.
pub fn webhook(s: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut r = Rules::on(s);
    url(&mut r, s, "url", "Url", true);
    r.done()
}
