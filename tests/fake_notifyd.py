#!/usr/bin/env python3
"""Minimal org.freedesktop.Notifications server for tests on a private bus.

Logs each Notify/CloseNotification call as a JSON line on stdout. Run it only
on a private session bus (aiws env), never on the owner's.
"""
import json
import sys

from gi.repository import Gio, GLib

XML = """
<node><interface name="org.freedesktop.Notifications">
  <method name="Notify">
    <arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
    <arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/>
    <arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/>
  </method>
  <method name="CloseNotification"><arg type="u" direction="in"/></method>
  <method name="GetCapabilities"><arg type="as" direction="out"/></method>
  <method name="GetServerInformation">
    <arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/>
  </method>
  <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
  <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
</interface></node>
"""
next_id = [0]


def call(conn, sender, path, iface, method, params, inv):
    if method == "Notify":
        app, replaces, icon, summary, body, actions, hints, timeout = params.unpack()
        next_id[0] += 1
        nid = replaces or next_id[0]
        print(json.dumps({"notify": {"id": nid, "app": app, "summary": summary, "body": body, "actions": actions}}), flush=True)
        inv.return_value(GLib.Variant("(u)", (nid,)))
    elif method == "CloseNotification":
        print(json.dumps({"close": params.unpack()[0]}), flush=True)
        inv.return_value(None)
    elif method == "GetCapabilities":
        inv.return_value(GLib.Variant("(as)", (["body", "actions", "persistence"],)))
    else:
        inv.return_value(GLib.Variant("(ssss)", ("fake_notifyd", "test", "0", "1.2")))


def acquired(conn, name):
    node = Gio.DBusNodeInfo.new_for_xml(XML)
    conn.register_object("/org/freedesktop/Notifications", node.interfaces[0], call, None, None)


def got_name(conn, name):
    print(json.dumps({"ready": name}), flush=True)


Gio.bus_own_name(Gio.BusType.SESSION, "org.freedesktop.Notifications", Gio.BusNameOwnerFlags.NONE,
                 acquired, got_name, lambda *a: sys.exit("name lost"))
GLib.MainLoop().run()
