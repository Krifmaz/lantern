package dev.krifmaz.lantern.benchmark

import androidx.benchmark.macro.BaselineProfileMode
import androidx.benchmark.macro.CompilationMode
import androidx.benchmark.macro.FrameTimingMetric
import androidx.benchmark.macro.StartupMode
import androidx.benchmark.macro.StartupTimingMetric
import androidx.benchmark.macro.junit4.MacrobenchmarkRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The app's journeys, measured on a release build: cold start without and with the Baseline Profile (the profile
 * should win), and the frame times of scrolling the home page.
 */
@RunWith(AndroidJUnit4::class)
class AppBenchmarks {
    /** The rule that runs and measures. */
    @get:Rule
    val rule = MacrobenchmarkRule()

    /** Cold start with nothing precompiled. */
    @Test
    fun startupNoProfile() = startup(CompilationMode.None())

    /** Cold start with the Baseline Profile installed (fails if the APK has none). */
    @Test
    fun startupBaselineProfile() = startup(CompilationMode.Partial(BaselineProfileMode.Require))

    /** Frame times while scrolling the home page, with the Baseline Profile. */
    @Test
    fun scroll() = rule.measureRepeated(
        packageName = APP_PACKAGE,
        metrics = listOf(FrameTimingMetric()),
        compilationMode = CompilationMode.Partial(BaselineProfileMode.Require),
        iterations = 5,
        startupMode = StartupMode.WARM,
        setupBlock = {
            pressHome()
            startActivityAndWait()
            openHome()
        },
    ) {
        scrollHome()
    }

    private fun startup(mode: CompilationMode) = rule.measureRepeated(
        packageName = APP_PACKAGE,
        metrics = listOf(StartupTimingMetric()),
        compilationMode = mode,
        iterations = 10,
        startupMode = StartupMode.COLD,
        setupBlock = { pressHome() },
    ) {
        startActivityAndWait()
    }
}
