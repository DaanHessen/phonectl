# phonectl ecosystem research: KDE Connect D-Bus vs. own protocol implementation, and ADB scope

Date: 2026-09-14
Scope: Nothing Phone 2 integration for `phonectl` (daemon + CLI + Waybar module) on Arch Linux, laptop running `kdeconnectd` 26.08.1, paired (protocol v8) with device id `7f58362b429c4839ba060ed698dfac74` ("Nothing Phone 2"), currently unreachable. `scrcpy` 4.1 and `android-tools` (adb 37) installed.

Confidence key: **[V]** verified against primary source in this session (source code read directly, or live D-Bus introspection on this machine) · **[S]** stated by a search-engine summary of a primary source, not independently re-read line-by-line · **[U]** unverified / inference, flagged explicitly.

---

## 1. Local D-Bus surface, verified live on this machine

Live introspection (read-only: `busctl --user list`, `busctl --user tree`, `busctl --user introspect`, and one no-side-effect method call `loadedPlugins`) against the running `kdeconnectd` (pid 2385) on the session bus. **[V]**

```
$ busctl --user list | grep -i kdeconnect
org.kde.kdeconnect            2385 kdeconnectd daanh :1.30 user@1000.service - -
org.kde.kdeconnect.daemon     2385 kdeconnectd daanh :1.30 user@1000.service - -

$ busctl --user tree org.kde.kdeconnect
├─ /MainApplication
├─ /modules
│ └─ /modules/kdeconnect
│   └─ /modules/kdeconnect/devices
│     └─ /modules/kdeconnect/devices/7f58362b429c4839ba060ed698dfac74
└─ /org
  └─ /org/kde
    └─ /org/kde/kdeconnect
      └─ /org/kde/kdeconnect/daemon
```

Both `org.kde.kdeconnect` and `org.kde.kdeconnect.daemon` are well-known names owned by the same process; **use `org.kde.kdeconnect`** as the bus name when connecting (that's what GSConnect/Valent target and what `qdbus`/`busctl` resolve by default).

Device object `/modules/kdeconnect/devices/7f58362b429c4839ba060ed698dfac74` exposes interface `org.kde.kdeconnect.device` with (confirmed live):

- Methods: `acceptPairing()`, `cancelPairing()`, `encryptionInfo() -> s`, `hasPlugin(s) -> b`, `isPairRequested() -> b`, `isPairRequestedByPeer() -> b`, `isPaired() -> b`, `isPluginEnabled(s) -> b`, `loadedPlugins() -> as`, `pairStateAsInt() -> i`, `pluginIconName(s) -> s`, `pluginsConfigFile() -> s`, `reloadPlugins()`, `requestPairing()`, `setPluginEnabled(s,b)`, `unpair()`, `verificationKey() -> s`.
- Properties (`emits-change`): `iconName`, `isPairRequested`, `isPairRequestedByPeer`, `isPaired` (currently `true`), `isReachable` (currently `false` — phone off/out of range right now), `name` (`"Nothing Phone 2"`), `pairState` (`3`), `statusIconName`, `supportedPlugins` (array of 29 plugin ids, see below), `type` (`"phone"`), `verificationKey`.
- Signals: `nameChanged(s)`, `pairStateChanged(i)`, `pairingFailed(s)`, `pluginsChanged()`, `reachableChanged(b)`, `statusIconNameChanged()`, `typeChanged(s)`.

`supportedPlugins` returned (this device, 29 entries) **[V]**:
```
kdeconnect_notifications kdeconnect_ping kdeconnect_pausemusic kdeconnect_findthisdevice
kdeconnect_bigscreen kdeconnect_findmyphone kdeconnect_sms kdeconnect_presenter
kdeconnect_systemvolume kdeconnect_remotecontrol kdeconnect_contacts kdeconnect_battery
kdeconnect_remotekeyboard kdeconnect_runcommand kdeconnect_connectivity_report
kdeconnect_lockdevice kdeconnect_sendnotifications kdeconnect_clipboard kdeconnect_telephony
kdeconnect_mmtelephony kdeconnect_mpriscontrol kdeconnect_share kdeconnect_mprisremote
kdeconnect_sftp kdeconnect_virtualmonitor kdeconnect_remotesystemvolume
kdeconnect_screensaver_inhibit kdeconnect_mousepad kdeconnect_remotecommands
```
**Important caveat, verified live**: `loadedPlugins()` currently returns an **empty array**, because the device is `isReachable == false` right now — plugin sub-objects (battery, notifications, mprisremote, etc.) are **only instantiated, and their D-Bus interfaces only exist, while the device is actually connected**. `supportedPlugins` is a static capability list computed from what the *phone last announced*, not a guarantee that the interface is live. Any client (phonectl included) must treat "plugin object path exists" as transient state that appears/disappears with `reachableChanged`/`pluginsChanged`, not something to cache long-term.

The daemon root object `org.kde.kdeconnect.daemon` interface introspects with a **duplicate-method warning** from `busctl` (`duplicate method 'sendSimpleNotification'`) **[V]** — harmless (it's a virtual/pure method declared in the base `Daemon` class and overridden in the desktop subclass; both show up in the introspection XML). Not a bug in phonectl's future client code, just noise to filter.

---

## 2. Full D-Bus API surface per plugin, from KDE Connect source (invent.kde.org, master branch, GPL-licensed)

Read directly from `plugins/*/*.h` on `invent.kde.org/network/kdeconnect-kde` (GitLab raw source, fetched via `-/raw/master/...` and the GitLab REST tree API — GitHub mirror's blob URLs 404 on raw fetch, GitLab API/raw worked). All `Q_CLASSINFO("D-Bus Interface", ...)` and `Q_SCRIPTABLE` members below are transcribed verbatim from the header files. **[V]**

All plugin objects live at `/modules/kdeconnect/devices/<deviceId>/<pluginDbusPath>` while the device is connected and the plugin loaded. `dbusPath()` per plugin (default pattern is `/modules/kdeconnect/devices/<id>/<name>` unless noted):

| Plugin id | D-Bus interface | Object path suffix | Notes |
|---|---|---|---|
| battery | `org.kde.kdeconnect.device.battery` | `/battery` | signal-driven |
| notifications | `org.kde.kdeconnect.device.notifications` | `/notifications` | signal-driven; per-notification sub-objects |
| notification (per-item) | `org.kde.kdeconnect.device.notifications.notification` | `/notifications/<internalId>` | request-based properties |
| mprisremote | `org.kde.kdeconnect.device.mprisremote` | `/mprisremote` | mixed |
| findthisdevice | `org.kde.kdeconnect.device.findthisdevice` | `/findthisdevice` | fire-and-forget, no properties |
| connectivity_report | `org.kde.kdeconnect.device.connectivity_report` | `/connectivity_report` | signal-driven |
| ping | `org.kde.kdeconnect.device.ping` | `/ping` | fire-and-forget |
| share | `org.kde.kdeconnect.device.share` | `/share` | request-based |
| clipboard | `org.kde.kdeconnect.device.clipboard` | `/clipboard` | signal + request |
| sms | `org.kde.kdeconnect.device.sms` | `/sms` | request-based, delegates conversation state |
| conversations (sub-interface of sms) | `org.kde.kdeconnect.device.conversations` | same sms plugin path | signal-driven for message stream |
| telephony | `org.kde.kdeconnect.device.telephony` | `/telephony` | signal-driven (call events) |
| remotecommands | `org.kde.kdeconnect.device.remotecommands` | `/remotecommands` | property + signal |
| sftp | `org.kde.kdeconnect.device.sftp` | `/sftp` (hardcoded, confirmed in source) | request-based (mount/unmount) |
| remotesystemvolume | `org.kde.kdeconnect.device.remotesystemvolume` | `/remotesystemvolume` | property + signal, phone's audio sinks controlled from desktop |
| remotekeyboard | `org.kde.kdeconnect.device.remotekeyboard` | `/remotekeyboard` | request + signal |
| virtualmonitor | `org.kde.kdeconnect.device.virtualmonitor` | `/virtualmonitor` | request-based |
| systemvolume (desktop-volume-from-phone) | **no D-Bus interface** | n/a | receives requests from phone to control *this machine's* volume; nothing to consume via D-Bus, only relevant if phonectl wants to *be* the receiver, which is irrelevant here |

Per-plugin detail (method / property / signal names exactly as declared):

**battery** — `Q_PROPERTY int charge`, `bool isCharging`, `bool hasBattery`, `QString iconName` (all `NOTIFY refreshed`); signal `refreshed(bool isCharging, int charge)`. No request method — this is purely push/signal-driven from the phone; a client just needs to read properties + subscribe to `refreshed`. Packet type on the wire: `kdeconnect.battery`. **[V]**

**notifications** — interface `org.kde.kdeconnect.device.notifications`:
- Methods: `activeNotifications() -> QStringList` (list of publicIds), `sendReply(replyId, message)`, `sendAction(key, action)`.
- Signals: `notificationPosted(publicId)`, `notificationRemoved(publicId)`, `notificationUpdated(publicId)`, `allNotificationsRemoved()`.
- Per-notification object (`.../notifications/<publicId>` conceptually — actual sub-object path built via `internalId`), interface `org.kde.kdeconnect.device.notifications.notification`: properties `internalId`, `appName`, `ticker`, `title`, `text`, `groupName`, `isConversation`, `isGroupConversation`, `iconPath`, `dismissable`, `hasIcon`, `silent`, `replyId` (all `NOTIFY ready`); methods `dismiss()`, `reply()`, `sendReply(message)`; signals `ready()`, plus internal (non-scriptable) `dismissRequested`, `replyRequested`, `actionTriggered`, `replied`.
- Fully signal-driven for arrival; each notification requires a follow-up property fetch (`GetAll` on the notification's own object) since `ready()` fires once metadata is populated. **[V]**

**mprisremote** (phone's media session, controlled from desktop) — properties `volume` (RW), `length`, `isPlaying`, `position` (RW), `playerList`, `player` (RW), `title`, `artist`, `album`, `localAlbumArtUrl`, `canSeek` (all except `player`/`localAlbumArtUrl` NOTIFY `propertiesChanged`); methods `seek(offset)`, `requestPlayerList()`, `sendAction(action)`; signal `propertiesChanged()`. Note the comment in source: album art fetching (`requestAlbumArt`) is deliberately *not* exposed via D-Bus. Mixed signal+request: call `requestPlayerList()` once, then react to `propertiesChanged`. **[V]**

**findthisdevice** — no properties, no signals; the plugin's only purpose is receiving `kdeconnect.findmyphone.request`. Looking at the header there is **no Q_SCRIPTABLE method to trigger it from D-Bus** in this file — triggering "find my phone" from the desktop appears to be done elsewhere (likely a UI action that calls into the plugin directly rather than via a dedicated D-Bus method, or via the `MainApplication`/tray applet, not the plugin's own D-Bus interface). **[U] — needs confirmation**: I did not find the actual send-trigger call site in the files fetched this session; do not assume a `findthisdevice` D-Bus method exists for triggering ringing from phonectl without checking `sendRequest`/plasmoid source further.

**connectivity_report** — properties `cellularNetworkType`, `cellularNetworkStrength`, `iconName` (NOTIFY `refreshed`); signal `refreshed(QString cellularNetworkType, int cellularNetworkStrength)`. Purely push-driven, packet `kdeconnect.connectivity_report`. **[V]**

**ping** — methods `sendPing()` and overload `sendPing(customMessage)`. No properties/signals on the D-Bus side (received pings surface as a desktop notification, not a D-Bus signal in this header). **[V]**

**share** — methods `shareUrl(url)`, `shareUrls(urls)`, `shareText(text)`, `openFile(file)`; signal `shareReceived(url)` (fired when *receiving* a share from the phone, giving the local destination path/URL). **[V]**

**clipboard** — property `isAutoShareDisabled` (NOTIFY `autoShareDisabledChanged`); methods `sendClipboard()` / `sendClipboard(content)`; signal `autoShareDisabledChanged(bool)`. Clipboard *content* itself is not exposed as a readable D-Bus property — it flows through the desktop's real clipboard (Qt `QClipboard`) once received, not through kdeconnect's D-Bus object. So "read remote clipboard via D-Bus" isn't directly possible; you'd read the system clipboard after a `kdeconnect.clipboard` packet arrives, or shell out to `wl-paste`/`xclip`. **[V]**

**sms / conversations** — `SmsPlugin` (`org.kde.kdeconnect.device.sms`): methods `sendSms(addresses, textMessage, attachmentUrls, subID=-1)`, `requestAllConversations()`, `requestConversation(conversationID, rangeStartTimestamp=-1, numberToRequest=-1)`, `launchApp()`, `requestAttachment(partID, uniqueIdentifier)`, `getAttachment(partID, uniqueIdentifier)`. Actual conversation/message data lives on a separate adaptor `ConversationsDbusInterface` (`org.kde.kdeconnect.device.conversations`, registered on the same object path as the sms plugin): methods `activeConversations() -> QVariantList`, `requestConversation(conversationID, start, end)`, `replyToConversation(conversationID, message, attachmentUrls)`, `sendWithoutConversation(addressList, message, attachmentUrls)`, `requestAllConversationThreads()`, `requestAttachmentFile(partID, uniqueIdentifier)`; signals `conversationCreated(QDBusVariant msg)`, `conversationRemoved(conversationID)`, `conversationUpdated(QDBusVariant msg)`, `conversationLoaded(conversationID, messageCount)`, `attachmentReceived(filePath, fileName)`. This is the most complex plugin surface; needs explicit `requestAllConversationThreads()`/`requestConversation()` calls, then react to signals streaming results back — not simple property reads. **[V]**

**telephony** — signal `callReceived(event, number, contactName)` where `event` is `"ringing"` or `"missedCall"` (SMS handling moved to the `sms` plugin). No caller-facing methods besides internal mute handling (`sendMutePacket` is a private slot, triggered by desktop notification action, not exposed on D-Bus). **[V]**

**remotecommands** — properties `commands` (QByteArray, JSON-encoded command list, NOTIFY `commandsChanged`), `deviceId` (constant), `canAddCommand` (constant); methods `triggerCommand(key)`, `editCommands()`; signal `commandsChanged(commands)`. This runs *desktop-defined* commands the phone can trigger remotely — not directly useful for "run a command on the phone from the desktop" (that direction doesn't exist in this plugin; it's the reverse). **[V]**

**sftp** — path hardcoded to `/modules/kdeconnect/devices/<id>/sftp` in source (only plugin observed to hardcode rather than use the generic pattern). Methods: `startBrowsing() -> bool`, `mount()`, `unmount()`, `mountAndWait() -> bool`, `isMounted() -> bool`, `getMountError() -> QString`, `mountPoint() -> QString`, `getDirectories() -> QVariantMap`; signals `mounted()`, `unmounted()`. This is a GVFS/FUSE mount of the phone's storage over the kdeconnect SFTP subsystem — a heavier, KIO/GIO-dependent path; likely **not worth consuming for phonectl**, since MTP via `gio`/`gvfs-mtp` or `adb` gives simpler raw file access without needing a paired, reachable kdeconnect session. **[V]**

**remotesystemvolume** — (phone's audio sinks, controlled from desktop) properties `sinks` (QByteArray, JSON, NOTIFY `sinksChanged`), `deviceId` (constant); methods `sendVolume(name, volume)`, `sendMuted(name, muted)`; signals `sinksChanged()`, `volumeChanged(name, volume)`, `mutedChanged(name, muted)`. **[V]**

**remotekeyboard** — property `remoteState` (NOTIFY `remoteStateChanged`); methods `sendKeyPress(key, specialKey=0, shift=false, ctrl=false, alt=false, super=false, sendAck=true)`, `sendQKeyEvent(keyEventMap, sendAck=true)`, `translateQtKey(qtKey)`; signal `keyPressReceived(...)` (phone sending characters *to* desktop), `remoteStateChanged(state)`. **[V]**

**virtualmonitor** — properties `lastError`, `isVirtualMonitorAvailable` (constant — depends on local `krfb`/RDP capability), `active` (NOTIFY `activeChanged`); methods `requestVirtualMonitor() -> bool`, `stop()`; signal `activeChanged()`. Requires local RDP/virtual-monitor support (KRDP/krfb) — **not applicable without a full Plasma RDP stack**; low priority for phonectl. **[V]**

**systemvolume** (`plugins/systemvolume/systemvolumeplugin-pulse.h`) — **no `Q_CLASSINFO("D-Bus Interface", ...)` at all** in the class declaration; this plugin only *receives* `kdeconnect.systemvolume`/`kdeconnect.systemvolume.request` packets to adjust the local (desktop) PulseAudio sinks when the phone asks. There is nothing to read here as a D-Bus consumer for "control phone volume from the desktop" — that's `remotesystemvolume` (above). **[V]**

### Daemon-level interface (`org.kde.kdeconnect.daemon`, from `core/daemon.h`)

```
Q_CLASSINFO("D-Bus Interface", "org.kde.kdeconnect.daemon")
Q_PROPERTY pairingRequests (QStringList, NOTIFY pairingRequestsChanged)
Q_PROPERTY customDevices   (QStringList, NOTIFY customDevicesChanged)

selfId() -> QString

forceOnNetworkChange()
announcedName() -> QString ; setAnnouncedName(QString)
devices(onlyReachable=false, onlyPaired=false) -> QStringList   // device ids
deviceNames(onlyReachable=false, onlyPaired=false) -> QMap<QString,QString>
deviceIdByName(name) -> QString
linkProviders() -> QStringList         // e.g. "BluetoothLinkProvider|enabled"
setLinkProviderState(linkProvider, enabled)
sendSimpleNotification(eventId, title, text, iconName)   // pure-virtual base; the duplicate busctl warning comes from this

Signals:
deviceAdded(id) / deviceRemoved(id) / deviceVisibilityChanged(id, isVisible) / deviceListChanged()
announcedNameChanged(name) / pairingRequestsChanged() / linkProvidersChanged(providers) / customDevicesChanged(devices)
```
`deviceRemoved` explicitly never fires for paired devices per the source comment — only unpaired/ephemeral devices disappear from the list; a paired-but-unreachable device (our current state) stays enumerable via `devices()`/`deviceNames()` with `isReachable=false`. **[V]** This matches what we observed live: the phone is still listed even though unreachable.

`forceOnNetworkChange()` is the method to call after a network change (Wi-Fi SSID switch, VPN toggle, resume from suspend) to force re-broadcast/reconnect — this is the one **state-changing call phonectl will plausibly want to invoke** (e.g. on a systemd-networkd/NetworkManager dispatcher hook, or after resume) rather than reimplementing discovery. Not invoked in this research session per the read-only constraint.

Discovery mechanism **[S, from general KDE Connect / Valent documentation, not re-verified against `backends/lan` source this session]**: KDE Connect's LAN backend broadcasts/listens on **UDP port 1716** (identity packets) and negotiates a **TCP connection on ports 1716–1764** for the actual TLS-wrapped packet stream; it also advertises via mDNS as `_kdeconnect._udp` on some builds/backends. Given the port range collision, **two independent KDE Connect protocol implementations cannot both bind port 1716 on the same host at the same time** — this directly answers part of item 4 below.

---

## 3. Android app battery/power behavior

- **[S]** KDE bug **442782** ("KDE Connect is draining battery") documents real user complaints: some report 30–50% battery usage attributable to the app. A KDE developer comment in that thread states plainly: *"KDE Connect daemon on Linux uses 10 second keep-alive interval on TCP socket"* — i.e. the desktop daemon keeps the long-lived TCP connection alive with **10-second TCP keepalive pings**, which prevents the phone's radio/CPU from reaching deep sleep, at least in older releases.
- **[V]** Merge request **`!447` "Increase TCP Keep-Alive initial interval to 300 seconds"** on `invent.kde.org/network/kdeconnect-kde` exists and targets exactly this — confirms the keepalive-interval-causes-battery-drain diagnosis was accepted by upstream and (partially) fixed by lengthening the interval from 10s to 300s. I did not verify from source whether this MR has landed on the specific 26.08.1 release the user runs; treat "300s keepalive" as the *direction* of the fix, not a guaranteed-current value without checking the changelog for 26.08.1 specifically.
- **[S]** The same bug thread surfaces a second, unrelated cause: **misbehaving MPRIS clients** (e.g., VLC emitting `Seeked` D-Bus signals ~4×/second) get relayed by the `mpriscontrol`/`mprisremote` plugin as one TCP packet per signal, each of which wakes the phone's radio — i.e. **any of phonectl's own future MPRIS-adjacent behavior needs to rate-limit signal relaying**, not just rely on kdeconnect's own throttling.
- **[U]** Whether the phone maintains the socket via a persistent foreground service (visible notification) or a background service killable by OEM battery optimization (Nothing OS, AOSP-based) was not independently confirmed this session; KDE Connect Android is known generally (community consensus, not verified here) to run as a foreground service specifically to survive Doze/App Standby, which is why it shows a permanent notification — consistent with, but not proven by, sources read this session.
- **Implication for phonectl**: consuming the existing D-Bus session is strictly better for phone battery than adding a second competing long-lived connection (see §4) — kdeconnectd already pays whatever keepalive cost exists, once, regardless of how many local consumers (Waybar, CLI, etc.) subscribe to its D-Bus signals.

---

## 4. Alternatives and the "reimplement the protocol" question

**Should phonectl speak the KDE Connect wire protocol itself, or consume kdeconnectd over D-Bus? → Consume over D-Bus.** Reasoning, in order of weight:

1. **Port/identity conflict is real and verified structurally** (§2): the LAN backend binds UDP 1716 and a TCP range for its own identity; a second daemon on the same host advertising the *same paired identity* to the phone would either fail to bind, or worse, race kdeconnectd for the connection, causing flapping. GSConnect and KDE Connect desktop are documented **[S]** as unable to run simultaneously on the same host for exactly this reason (port ownership), and Valent's own discussion (`andyholmes/valent#240`, **[S]**, not fully re-read) explicitly grapples with "how do we coexist with kdeconnect/gsconnect." Any second implementation is fighting the same problem, not avoiding it.
2. **Re-pairing cost.** The phone is already paired (TLS certificate trust established) with kdeconnectd's specific device identity/keypair. A separate phonectl implementation would need its own identity and would require **re-pairing the phone** (a manual on-phone confirmation tap) — not acceptable for a tool meant to sit invisibly behind Waybar.
3. **Battery cost is not additive when reusing kdeconnectd.** Per §3, whatever keepalive/socket cost exists is paid once by kdeconnectd regardless of how many local D-Bus consumers subscribe — reimplementing the protocol would add a **second parallel persistent connection** with its own keepalive, roughly doubling the phone-side wake-up cost for the features it duplicates.
4. **Maintenance cost.** The KDE Connect wire protocol (TLS handshake specifics, packet framing, per-plugin JSON schemas, SFTP/file-transfer chunking) is nontrivial and evolves (protocolVersion 8 currently); D-Bus method/signal names are comparatively stable and versioned by KDE's own API stability policy for installed system daemons.
5. **What D-Bus *can't* give you** is the one legitimate argument for going direct-protocol: plugins/features kdeconnectd doesn't expose at all over D-Bus (e.g. raw clipboard *content*, §2) or a desire to run without KDE Connect installed at all (e.g. targeting a machine with only GSConnect/Valent, or none). None of those apply here — the user already runs and depends on kdeconnectd.

**Rust KDE Connect implementations found (for context, not needed given the above):**
- `kdeconnect-proto` (docs.rs) — pure protocol/framing crate (packet types, tokio backend, an Embassy/embedded backend). **[S]**, not independently inspected.
- `cosmic-ext-connect-core` (GitHub, `olafkfreund`) — Rust KDE Connect protocol v7 core shared by a COSMIC Desktop applet and a Kotlin Android app ("COSMIC Connect"). Protocol v7 is behind the phone's negotiated v8; unclear compatibility/maintenance state. **[S]**
- `cosmic-utils/kdeconnect` — a native Rust KDE Connect implementation for COSMIC Desktop. **[S]**, appears to be a separate, competing daemon (same port-conflict problem as above) rather than a library you'd embed.
- `rust-connect` (georgeglarson) — "API-first" reimplementation, explicitly designed to pair with the stock Android app (i.e., its own identity, so same re-pairing problem). **[S]**
- `r58Playz/kdeconnect` — targets jailbroken iOS, off-topic for this use case. **[S]**
- None of these were evaluated for license or activity level beyond the search-result summaries above; if reimplementation is ever reconsidered (e.g. multi-device future where D-Bus's single-daemon model becomes limiting), `kdeconnect-proto` is the most promising building block to re-examine, since it claims to be framing-only rather than a competing full daemon.

**GSConnect** (GNOME Shell extension, GPL-2.0, **[S]** not re-verified) and **Valent** (GTK/libadwaita, GPL-3.0, **[S]**) are both complete alternative daemons with the same "can't run alongside kdeconnect-kde" constraint — irrelevant to phonectl's design since the user already has kdeconnectd as the one daemon; phonectl should be a *client*, not a competing daemon, regardless of which of these three implementations happens to own the port.

---

## 5. Other reusable Linux↔Android projects (brief)

| Project | What it provides | License | Relevance |
|---|---|---|---|
| **scrcpy** 4.1 (Genymobile) | USB/TCP-IP screen mirroring + input control, no root; already installed | Apache-2.0 **[V/S]** (GitHub `LICENSE` file confirmed Apache-2.0 by search result quoting the file directly) | Directly usable as a subprocess for a "mirror screen" phonectl command; permissive license, trivial to shell out to. |
| **android-tools / adb** 37 | USB/TCP debug bridge: shell, file push/pull, `dumpsys battery`, package management, port forwarding, `screenrecord` | Apache-2.0 (AOSP) **[U]** not independently checked this session, but AOSP platform tools are Apache-2.0 by long-standing convention | Best fit for anything scrcpy/kdeconnect don't cover: precise battery stats (`dumpsys battery`), app-level automation, notification listing via `dumpsys notification`, install/uninstall, logs. Requires USB or `adb tcpip` Wi-Fi debugging (separate trust model from kdeconnect pairing — a second "pairing" the user has to do once via USB or QR). |
| **LocalSend** | Cross-platform (Linux/Android/etc.) LAN file/text sharing, mDNS discovery, HTTPS-based, no account | Apache-2.0 **[S]** | Overlaps with kdeconnect's `share`/`sftp` plugins; not needed given kdeconnect already covers file share, but a fallback if kdeconnect pairing is ever broken. |
| **Warpinator** (Linux Mint) + unofficial Android port | LAN file transfer, gRPC-based, Linux-Mint-flavored UI | GPL-3.0 for the Android port **[S]**; upstream Warpinator is GPL-3.0 as well (not independently re-checked) | Same category as LocalSend, lower relevance — Mint-specific ecosystem, unofficial Android client. |
| **MTP tooling** (`gvfs-mtp`/`jmtpfm`/`simple-mtpfs`) | Raw filesystem access to Android storage over USB without ADB or kdeconnect | Mostly LGPL/GPL depending on tool | Only relevant if kdeconnect's `sftp` plugin (needs live pairing) and ADB (needs USB debugging) are both unavailable; low priority. |

---

## 6. License notes for phonectl's design

- **kdeconnect-kde**: SPDX headers actually found in every plugin header this session read as **`GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL`** **[V]** — this is a *triple-license choice*, not the single "GPL-2.0-or-later" assumed going in. Correction noted: don't cite it as "GPL-2.0-or-later" in phonectl's own docs; cite the actual SPDX expression, or just "GPL, KDE Accepted GPL variant" generically. This matters only if phonectl vendors any kdeconnect source/headers directly (e.g. copying packet-type string constants) — **consuming a running daemon over D-Bus (a runtime IPC boundary) does not trigger GPL's copyleft on phonectl itself**, regardless of phonectl's own license, the same way no CLI tool that D-Bus-calls `systemd` or `NetworkManager` becomes GPL. This is standard practice (GSConnect itself is GPL but that's a design choice, not the only legally required option for a D-Bus client) and not legal advice — flag for the user if phonectl's own license needs to interoperate with anything GPL-copied verbatim (e.g. literal packet-type string constants like `"kdeconnect.battery"` are just protocol identifiers, not copyrightable expression, and safe to reuse regardless).
- **scrcpy**: Apache-2.0. Shelling out to the `scrcpy` binary as a subprocess imposes **no license obligations** on phonectl beyond normal Apache-2.0 redistribution notices if phonectl ever bundles the scrcpy binary itself (it won't — it's a system dependency the user already has installed).
- **android-tools/adb**: Apache-2.0 (AOSP convention). Same subprocess-boundary reasoning as scrcpy.
- **Net effect**: phonectl can be any license the user wants (the memory files imply this is Daan's own tool, likely to stay unlicensed/private or whatever he picks) while consuming kdeconnectd via D-Bus and shelling out to scrcpy/adb, with no forced copyleft from either dependency given the process/IPC boundary. Flag this reasoning as **non-legal-advice** if this doc is ever the basis for an actual public release decision.

---

## Recommendations for phonectl's architecture (synthesis, not sourced beyond the above)

1. **Do not implement the KDE Connect wire protocol.** Consume `org.kde.kdeconnect` over D-Bus for: battery, connectivity_report, notifications (list + signals + reply/dismiss), mprisremote (media control/status), findmyphone (module send-trigger location TBD, see caveat in §2), ping, share, clipboard-signal-triggering (but read actual clipboard content from the system clipboard, not kdeconnect's D-Bus), sms/conversations, telephony (call events), remotecommands (only if phonectl wants to expose desktop-triggerable commands to the phone, not the reverse).
2. **Skip via D-Bus**: `sftp` (heavier GVFS mount machinery; prefer ADB or MTP for file access), `virtualmonitor` (needs full KRDP/krfb stack, not installed), `remotesystemvolume`/`systemvolume` (low value for a phone-integration tool unless "control phone volume from Waybar" is an explicit feature request).
3. **Use ADB for**: precise battery telemetry (`dumpsys battery` gives more detail than kdeconnect's coarse charge%/isCharging), notification *listing* cross-check or richer detail (`dumpsys notification`), any scripted actions kdeconnect doesn't expose (app launch, input injection beyond keyboard, screenshots/`screenrecord`), and as a fallback path when kdeconnect is unreachable but the phone is on USB or `adb tcpip`.
4. **Use scrcpy for**: an on-demand "mirror/control screen" command — shell out, don't reimplement.
5. **Reconnection**: call `org.kde.kdeconnect.daemon`'s `forceOnNetworkChange()` from a network-change hook (NetworkManager dispatcher script or systemd-networkd hook) rather than polling `isReachable`; subscribe to `reachableChanged`/`deviceListChanged`/`pluginsChanged` signals for reactive Waybar updates instead of polling properties.
6. **Open item to verify before implementation**: the exact D-Bus (or non-D-Bus) call path for triggering "find my phone" from the desktop side — the `findthisdevice` plugin header exposes no scriptable trigger method; check `plasmoid`/`indicator`/`cli` source under the same GitLab repo for how the existing tray applet/`kdeconnect-cli` invokes it (likely via `sendRequest`/internal signal rather than a public D-Bus method, or possibly it genuinely has none and the feature is UI-only inside the KCM/plasmoid — needs a follow-up source read of `cli/kdeconnect-cli.cpp` and `plasmoid/package/contents/**` before phonectl commits to a design assuming a D-Bus call exists).

## Sources

- Live introspection: `busctl --user list|tree|introspect|call` against local `kdeconnectd` (this session).
- KDE Connect source (GPL, invent.kde.org/network/kdeconnect-kde, master branch, fetched via GitLab raw/API this session): `core/daemon.h`, `plugins/battery/batteryplugin.h`, `plugins/notifications/{notificationsplugin.h,notification.h}`, `plugins/mprisremote/mprisremoteplugin.h`, `plugins/findthisdevice/findthisdeviceplugin.h`, `plugins/connectivity-report/connectivity_reportplugin.h`, `plugins/ping/pingplugin.h`, `plugins/share/shareplugin.h`, `plugins/clipboard/clipboardplugin.h`, `plugins/sms/{smsplugin.h,conversationsdbusinterface.h}`, `plugins/telephony/telephonyplugin.h`, `plugins/remotecommands/remotecommandsplugin.h`, `plugins/sftp/sftpplugin.h`, `plugins/remotesystemvolume/remotesystemvolumeplugin.h`, `plugins/remotekeyboard/remotekeyboardplugin.h`, `plugins/virtualmonitor/virtualmonitorplugin.h`, `plugins/systemvolume/systemvolumeplugin-pulse.h`, `REUSE.toml`.
- [KDE bug 442782 — "KDE Connect is draining battery"](https://bugs.kde.org/show_bug.cgi?id=442782)
- [MR !447 — Increase TCP Keep-Alive initial interval to 300 seconds](https://invent.kde.org/network/kdeconnect-kde/-/merge_requests/447)
- [Valent protocol reference](https://valent.andyholmes.ca/documentation/protocol.html)
- [Valent discussion #240 — coexistence with kdeconnect/gsconnect](https://github.com/andyholmes/valent/discussions/240)
- [Genymobile/scrcpy](https://github.com/genymobile/scrcpy) — Apache-2.0 LICENSE confirmed via search snippet
- [LocalSend](https://github.com/localsend/localsend) — Apache-2.0
- [cosmic-ext-connect-core](https://github.com/olafkfreund/cosmic-ext-connect-core), [cosmic-utils/kdeconnect](https://github.com/cosmic-utils/kdeconnect), [rust-connect](https://github.com/georgeglarson/rust-connect), [kdeconnect-proto docs.rs](https://docs.rs/kdeconnect-proto/latest/kdeconnect_proto/), [r58Playz/kdeconnect](https://github.com/r58Playz/kdeconnect)
