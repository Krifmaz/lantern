package dev.krifmaz.lantern.baselineprofile

import androidx.benchmark.macro.junit4.BaselineProfileRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Generates the app's Baseline Profile and startup profile: a cold start, then scrolling the home page. */
@RunWith(AndroidJUnit4::class)
class BaselineProfileGenerator {
    /** The rule that collects the profile. */
    @get:Rule
    val rule = BaselineProfileRule()

    /** Start and scroll; the start-up part also becomes the startup profile (DEX layout). */
    @Test
    fun generate() = rule.collect(packageName = APP_PACKAGE, includeInStartupProfile = true) {
        pressHome()
        startActivityAndWait()
        openHome()
        scrollHome()
    }
}
