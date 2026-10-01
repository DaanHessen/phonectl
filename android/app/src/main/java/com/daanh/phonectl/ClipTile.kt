package com.daanh.phonectl

import android.app.PendingIntent
import android.content.Intent
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/** Quick Settings tile: send the current clipboard to the laptop. */
class ClipTile : TileService() {
    override fun onStartListening() {
        qsTile?.apply {
            state = if (Link.state == Link.State.CONNECTED) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
            subtitle = when (Link.state) {
                Link.State.CONNECTED -> Link.session?.transport?.wire
                Link.State.CONNECTING -> "connecting"
                Link.State.UNPAIRED -> "not paired"
                else -> "offline"
            }
            updateTile()
        }
    }

    override fun onClick() {
        val intent = Intent(this, ClipReadActivity::class.java)
            .putExtra("source", "tile")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)
        startActivityAndCollapse(PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE))
    }
}
