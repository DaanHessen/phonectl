# Device observations

Everything here was observed on the real phone over wireless debugging on
2026-09-14. Raw dumps sit next to this file in `device/`.

## Identity

| Property | Value |
|---|---|
| Marketing name | Nothing Phone (4a) Pro (`settings global device_name`) |
| `ro.product.model` | A069P |
| `ro.product.device` | FroggerPro |
| `ro.product.name` | FroggerProEEA |
| `ro.board.platform` | sun |
| `ro.soc.model` | SM7750 (Qualcomm) |
| Android | 16, SDK 36, security patch 2026-07-01 |
| Build | B4.1-260723-1820 (Nothing OS 4.1) |

The original brief names the Nothing Phone 2. The phone actually in use is the
Phone (4a) Pro. The "Nothing Phone 2" entry in KDE Connect is an older phone.

`ro.build.nothing.feature.diff.device.<Codename>` props exist for Asteroids,
Frogger, FroggerPro, Galaga, Galaxian and more: one bitmask per sibling device,
shipped in every build. Useful for a generic "is this a Nothing device" check,
not decoded.

## Wireless debugging

- `adb pair 192.168.1.20:41161 <code>` worked. The GUID is `adb-<serial>-<suffix>`.
- Right after pairing the phone advertised `_adb-tls-connect._tcp` on port
  39615, and the stock adb server auto-connected. Before pairing, nothing was
  advertised even with wireless debugging on. Avahi also saw nothing, so this
  is how the phone behaves, not a discovery bug. Needs rechecking: does the phone only
  advertise once a paired host exists?
- `settings global adb_wifi_enabled=1`, `adb_enabled=0` (USB debugging off).

## Shell (uid 2000) capabilities that matter

`dumpsys package com.android.shell` shows these granted, among others:
`CONTROL_DEVICE_LIGHTS`, `MEDIA_CONTENT_CONTROL`, `MANAGE_NOTIFICATIONS`,
`MANAGE_NOTIFICATION_LISTENERS`, `STATUS_BAR_SERVICE`, `WRITE_SECURE_SETTINGS`,
`DUMP`, `READ_LOGS`, `BATTERY_STATS`, `READ_PHONE_STATE`, `ACCESS_WIFI_STATE`,
`NETWORK_SETTINGS`, `READ_CLIPBOARD_IN_BACKGROUND`, `PACKAGE_USAGE_STATS`,
`INTERACT_ACROSS_USERS_FULL`.

## Shell agent spike (`app_process`, no APK)

A dex pushed to `/data/local/tmp` and started with
`CLASSPATH=... app_process / <Main>` runs as uid 2000 with the grants above.
This is the scrcpy server model.

| Test | Result |
|---|---|
| `ILightsManager.getLights()` via ServiceManager | works, 22 lights (below) |
| `IActivityManager.registerReceiverWithFeature` (battery broadcasts) | **fails**: AMS logs `registerReceiverWithFeature: no app for null` and returns null. An `app_process` process has no ProcessRecord, so it can never receive broadcasts (AOSP `BroadcastController.java`, the `callerApp == null` check). |
| `BatteryManager` built by reflection over `batterystats` + `batteryproperties` binders | works: capacity 43, status 3 (discharging), current_now -552992 µA, isCharging false |
| `ISessionManager.getSessions(null, 0)` | works, lists active sessions |
| `ISessionManager.addSessionsListener(listener, null, 0)` | registers without error (`MEDIA_CONTENT_CONTROL` path in `MediaSessionService.verifySessionsRequest`) |
| `new MediaController(null, token)` | fails, needs a Context. |
| `Context.getSystemService(MediaSessionManager)` | fails: NPE in `MediaSessionManager.<init>`, because `MediaFrameworkInitializer` never ran in an `app_process` process. Avoid the manager class. |
| System context (ActivityThread + `mSystemThread`, as in scrcpy) wrapped as `com.android.shell`, tokens from `ISessionManager.getSessions`, `new MediaController(ctx, token)` | **works**: package, playback state, metadata (title/artist), `registerCallback` and `addSessionsListener` all succeed |

### Notifications

`INotificationManager.registerListener` only checks `STATUS_BAR_SERVICE`
(`NotificationManagerService.enforceSystemOrSystemUI`, Lineage 23 / Android 16
source), and shell holds it. A `NotificationListenerService` subclass
registered with the hidden `registerAsSystemService(ctx, component, user)`
from the shell agent (component `com.android.shell/phonectl.Listener`):

- `onListenerConnected` fired; `getActiveNotifications()` returned 68 entries
- a notification posted with `cmd notification post` arrived as
  `onNotificationPosted` within the same second
- `unregisterAsSystemService()` worked

So notifications are fully event-driven from shell, with no APK and no
notification-access grant on the phone. The spike logged package names and
ids only, never notification content.

## Lights

`dumpsys lights` and `getLights()` agree: ids 102-125 (type = id, vendor
range, ordinals 10004-10028), plus id 105 with ordinal 100 and id 500 with type
-12. These are almost certainly the Glyph zones. Not yet mapped to physical
positions. Nothing has been written to any light.

## Nothing packages of interest

- `com.nothing.thirdparty` 13.01.01: `.GlyphService`, action
  `com.nothing.thirdparty.bind_glyphservice`, guarded by
  `com.nothing.ketchum.permission.ENABLE` which is **protection level normal**.
  It requests `CONTROL_DEVICE_LIGHTS` itself.
- `com.nothing.glyphnotification`: Glyph Progress. It is a notification
  listener with `GlyphProgressBroadcastReceiver`s. `settings system
  glyph_progress_main_switch=0` (off).
- `com.nothinglondon.toys`: Glyph Toys, action `com.nothing.glyph.TOY`, holds
  `com.nothing.ketchum.permission.ENABLE`.
- Glyph settings keys (global): `led_effect_enable`, `led_brightness_value`,
  `led_auto_brightness_enable`, `led_effect_call_enalbe` (sic),
  `led_effect_charging_enable`, `led_effect_music_enalbe` (sic),
  `led_effect_volume_indicator_enable`, `led_bed_time_*`,
  `glyph_long_torch_enable`, `glyph_timer_*`, `glyph_pocket_mode_state`,
  `glyph_screen_upward_state`; secure: `glyph_pocket_enable`. QS tiles
  `glyphs`, `glyphs_torch`, Glyph Timer.
- `nt_wireless_reverse_charge`, `nt_reverse_charging_limiting_level` (global):
  reverse wireless charging state.

## Other

- KDE Connect Android (`org.kde.kdeconnect_tp`) and Microsoft Phone Link
  (`com.microsoft.appmanager`) are installed and hold notification listener
  access. We use neither.
