package dev.krifmaz.lantern.benchmark

import androidx.benchmark.macro.MacrobenchmarkScope
import androidx.test.uiautomator.By
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.Until

/** The app's package (the release build's). */
const val APP_PACKAGE = "dev.krifmaz.lantern"

/**
 * Gets to the home page: on a fresh install the app first asks for the server, so Continue is tapped with the
 * suggested address (run the server on the computer first: the emulator reaches it at 10.0.2.2:9696). Sign in once
 * by hand on the device before running: the app keeps its token, and the journeys don't type a password.
 */
fun MacrobenchmarkScope.openHome() {
    device.findObject(By.res("connect-continue"))?.let {
        it.click()
        device.wait(Until.hasObject(By.res("home-list")), 10_000)
    }
}

/**
 * Scrolls the home page (resource id "home-list") down and back up: the journey the profile covers and the scroll
 * benchmark measures.
 */
fun MacrobenchmarkScope.scrollHome() {
    device.wait(Until.hasObject(By.res("home-list")), 5_000)
    val list = device.findObject(By.res("home-list")) ?: return
    // keep the fling away from the system's edge gestures
    list.setGestureMargin(device.displayWidth / 5)
    list.fling(Direction.DOWN)
    device.waitForIdle()
    list.fling(Direction.UP)
    device.waitForIdle()
}
