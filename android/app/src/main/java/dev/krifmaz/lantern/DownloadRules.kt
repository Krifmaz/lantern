package dev.krifmaz.lantern

import dev.krifmaz.kit.compose.core.arr
import dev.krifmaz.kit.compose.core.dbl
import dev.krifmaz.kit.compose.core.formatSize
import dev.krifmaz.kit.compose.core.int
import dev.krifmaz.kit.compose.core.long
import dev.krifmaz.kit.compose.core.obj
import dev.krifmaz.kit.compose.core.objectsOf
import dev.krifmaz.kit.compose.core.str
import kotlinx.serialization.json.JsonObject

/** One download Lantern follows for an app (`GET /api/hub/grabs`'s `grabs`). */
data class Grab(
    /** The grab's id on the server. */
    val id: Long,
    /** The app it is for (its id). */
    val appId: Long,
    /** The app's name. */
    val app: String,
    /** The release's name. */
    val title: String,
    /** "torrent" or "usenet". */
    val protocol: String,
    /** queued, waiting, downloading, paused, seeding, completed, failed or removed. */
    val state: String,
    /** How far it is, 0 to 1. */
    val progress: Double,
    /** Its size in bytes (0 while not known). */
    val size: Long,
    /** Bytes a second now. */
    val speed: Long,
    /** Seconds left (0 while not known). */
    val eta: Long,
    /** What went wrong, in the server's words ("" when nothing did). */
    val error: String,
    /** How many times it was sent. */
    val attempts: Int,
    /** When it will be sent again (seconds since 1970; 0 for no new try). */
    val nextTry: Double,
    /** Where the files are, as the app sees them ("" until known). */
    val path: String,
)

/** One app connected to Lantern (`GET /api/hub/apps`'s `apps`). */
data class HubApp(
    /** The app's id on the server. */
    val id: Long,
    /** Its name ("Sonarr" …). */
    val name: String,
    /** What it handles ("tv", "movies", "music" …). */
    val media: String,
    /** Its own download category. */
    val category: String,
    /** Its folder as the app sees it. */
    val importPath: String,
    /** Why the last call back to it failed ("" when it worked or there is none). */
    val callbackError: String,
    /** When it last talked to Lantern (seconds since 1970; 0 for never). */
    val lastSeen: Double,
    /** Its grabs, counted by state. */
    val grabs: Map<String, Int>,
)

/** Which downloads the Downloads page shows. */
enum class GrabFilter(
    /** The choice's words. */
    val label: String,
) {
    /** Every download. */
    All("All"),

    /** Not finished yet: queued, waiting, downloading or paused. */
    Active("Active"),

    /** Finished: seeding or completed. */
    Done("Done"),

    /** Failed. */
    Failed("Failed"),
}

/** The download rules: reading the hub's answers, filters and the words shown. Plain Kotlin, unit-tested. */
object DownloadRules {
    /** The states of a download that isn't finished. */
    private val ACTIVE = setOf("queued", "waiting", "downloading", "paused")

    /** The states of a finished download. */
    private val DONE = setOf("seeding", "completed")

    /** The downloads of `GET /api/hub/grabs` (`{"grabs": [...]}`), newest first as the server gives them. */
    fun grabsOf(o: JsonObject?): List<Grab> = o?.arr("grabs").objectsOf().map {
        Grab(
            id = it.long("id"),
            appId = it.long("app_id"),
            app = it.str("app"),
            title = it.str("title").ifEmpty { "Download ${it.long("id")}" },
            protocol = it.str("protocol"),
            state = it.str("state"),
            progress = it.dbl("progress").coerceIn(0.0, 1.0),
            size = it.long("size"),
            speed = it.long("speed"),
            eta = it.long("eta"),
            error = it.str("error"),
            attempts = it.int("attempts"),
            nextTry = it.dbl("next_try"),
            path = it.str("path"),
        )
    }

    /** The apps of `GET /api/hub/apps` (`{"apps": [...]}`). */
    fun appsOf(o: JsonObject?): List<HubApp> = o?.arr("apps").objectsOf().map {
        val counts = it.obj("grabs")
        HubApp(
            id = it.long("id"),
            name = it.str("name").ifEmpty { "App ${it.long("id")}" },
            media = it.str("media"),
            category = it.str("category"),
            importPath = it.str("import_path"),
            callbackError = it.str("callback_error"),
            lastSeen = it.dbl("last_seen"),
            grabs = counts?.keys?.associateWith { k -> counts.int(k) }.orEmpty(),
        )
    }

    /** The downloads of [list] that [f] keeps, and of app [appId] only when it isn't null. Removed ones never show. */
    fun filter(list: List<Grab>, f: GrabFilter, appId: Long? = null): List<Grab> = list.filter { g ->
        g.state != "removed" &&
            (appId == null || g.appId == appId) &&
            when (f) {
                GrabFilter.All -> true
                GrabFilter.Active -> g.state in ACTIVE
                GrabFilter.Done -> g.state in DONE
                GrabFilter.Failed -> g.state == "failed"
            }
    }

    /** Whether [g] is still moving, so the page asks again soon. */
    fun busy(g: Grab): Boolean = g.state in ACTIVE

    /** A state's badge words and the kit's badge kind. */
    fun stateBadge(state: String): Pair<String, String> = when (state) {
        "queued" -> "Queued" to "info"
        "waiting" -> "Waiting" to "info"
        "downloading" -> "Downloading" to "accent"
        "paused" -> "Paused" to "warn"
        "seeding" -> "Seeding" to "ok"
        "completed" -> "Done" to "ok"
        "failed" -> "Failed" to "err"
        "removed" -> "Removed" to ""
        else -> state.replaceFirstChar { it.uppercase() } to ""
    }

    /** The line under a download: "Sonarr · 42% of 1.4 GB · 5.2 MB/s · 3 min left". */
    fun line(g: Grab): String {
        val parts = mutableListOf<String>()
        if (g.app.isNotEmpty()) parts += g.app
        val pct = (g.progress * 100).toInt()
        parts += when {
            g.state == "downloading" && g.size > 0 -> "$pct% of ${formatSize(g.size)}"
            g.state == "downloading" -> "$pct%"
            g.size > 0 -> formatSize(g.size)
            else -> g.protocol.replaceFirstChar { it.uppercase() }
        }
        if (g.state == "downloading" && g.speed > 0) parts += formatSize(g.speed) + "/s"
        if (g.state == "downloading" && g.eta > 0) parts += etaWords(g.eta) + " left"
        return parts.filter { it.isNotEmpty() }.joinToString(" · ")
    }

    /** What went wrong with [g] and what Lantern does next, at [now]; "" when nothing went wrong. */
    fun problemLine(g: Grab, now: Double): String {
        val why = g.error.trim().trimEnd('.')
        if (why.isEmpty()) return ""
        return when {
            g.state == "failed" -> "$why. Lantern gave up after ${if (g.attempts == 1) "1 try" else "${g.attempts} tries"}; tap Try again to send it again."
            g.nextTry > now -> "$why. Lantern tries again in ${etaWords((g.nextTry - now).toLong())}."
            else -> "$why."
        }
    }

    /** Seconds as people say them: "40 s", "3 min", "2 h 5 min", "3 days". */
    fun etaWords(seconds: Long): String {
        val s = seconds.coerceAtLeast(0)
        return when {
            s < 60 -> "$s s"
            s < 3600 -> "${s / 60} min"
            s < 86_400 -> if ((s % 3600) / 60 == 0L) "${s / 3600} h" else "${s / 3600} h ${(s % 3600) / 60} min"
            else -> if (s / 86_400 == 1L) "1 day" else "${s / 86_400} days"
        }
    }

    /** What an app handles, in words ("tv" → "TV"). */
    fun mediaWords(media: String): String = when (media.lowercase()) {
        "tv" -> "TV"
        "" -> ""
        else -> media.replaceFirstChar { it.uppercase() }
    }

    /** The line under an app: "TV · 3 active · 12 done · 1 failed"; "No downloads yet" for none. */
    fun appLine(a: HubApp): String {
        val active = ACTIVE.sumOf { a.grabs[it] ?: 0 }
        val done = DONE.sumOf { a.grabs[it] ?: 0 }
        val failed = a.grabs["failed"] ?: 0
        val counts = buildList {
            if (active > 0) add("$active active")
            if (done > 0) add("$done done")
            if (failed > 0) add("$failed failed")
        }
        val media = mediaWords(a.media)
        val tail = if (counts.isEmpty()) "No downloads yet" else counts.joinToString(" · ")
        return if (media.isEmpty()) tail else "$media · $tail"
    }
}
