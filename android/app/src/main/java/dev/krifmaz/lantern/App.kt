package dev.krifmaz.lantern

import android.app.Application
import coil3.ImageLoader
import dev.krifmaz.kit.compose.KitApp
import dev.krifmaz.kit.compose.bugs.BugReportsApi
import dev.krifmaz.kit.compose.bugs.CrashLog
import dev.krifmaz.kit.compose.bugs.CrashLogHost
import dev.krifmaz.kit.compose.core.KitApi
import dev.krifmaz.kit.compose.images.buildKitImageLoader
import dev.krifmaz.kit.compose.perf.KitPerf
import dev.krifmaz.kit.compose.update.UpdateRules
import dev.krifmaz.kit.compose.update.UpdateSource
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob

/**
 * The app process: sets up the kit once (crash log, the app's name and version, where updates come from) and holds
 * what every screen shares: the server's address, its API, the bug report calls and the image loader. It is also
 * the [CrashLogHost] that sends crash reports once a server is connected.
 */
class App :
    Application(),
    CrashLogHost {
    /** Which server, and who is signed in to it. */
    lateinit var settings: ServerSettings
        private set

    /** The server's API (the kit's parts use [ServerApi.kit]). */
    lateinit var server: ServerApi
        private set

    /** What's new from the server. */
    lateinit var changelog: ServerChangelog
        private set

    /** The app's image loader (bug report pictures); built on first use, through [ServerApi.http]. */
    val imageLoader: ImageLoader by lazy { buildKitImageLoader(this, server.http) }

    /** Lives as long as the app: crash reports are sent in it. */
    override val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /** The server's API for crash reports. */
    override val api: KitApi get() = server.kit

    private lateinit var bugs: BugReportsApi

    /** The bug report calls. */
    override val bugReports: BugReportsApi get() = bugs

    /** This phone's id, sent with each crash report. */
    override val deviceId: String get() = settings.deviceId

    /** Crash reports go to the server once the app is connected to one. */
    override suspend fun signedIn(): Boolean = settings.base.value.isNotEmpty()

    /** Sets up the kit and sends crash reports left from earlier runs. */
    override fun onCreate() {
        // first thing: catch crashes from here on
        CrashLog.install(this)
        KitPerf.strictModeInDebug(BuildConfig.DEBUG)
        super.onCreate()
        val name = getString(R.string.app_name)
        val userAgent = name.replace(" ", "") + "-Android/" + BuildConfig.VERSION_NAME
        KitApp.init(name = name, versionName = BuildConfig.VERSION_NAME, versionCode = BuildConfig.VERSION_CODE, userAgent = userAgent)
        UpdateRules.source = UpdateSource(BuildConfig.UPDATE_OWNER, BuildConfig.UPDATE_REPO, BuildConfig.UPDATE_APK_PREFIX, BuildConfig.RELEASE_PACKAGE)
        settings = ServerSettings(this)
        server = ServerApi(settings)
        changelog = ServerChangelog(server)
        bugs = BugReportsApi(server.kit, server.http)
        CrashLog.uploadPending(this, this)
    }
}
