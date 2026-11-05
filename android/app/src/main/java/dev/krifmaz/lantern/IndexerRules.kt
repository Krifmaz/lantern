package dev.krifmaz.lantern

import dev.krifmaz.kit.compose.core.arr
import dev.krifmaz.kit.compose.core.bool
import dev.krifmaz.kit.compose.core.dbl
import dev.krifmaz.kit.compose.core.int
import dev.krifmaz.kit.compose.core.long
import dev.krifmaz.kit.compose.core.objectsOf
import dev.krifmaz.kit.compose.core.str
import kotlinx.serialization.json.JsonObject

/**
 * One indexer as `GET /api/indexers` gives it: what the Indexers page and the home page's health line need. Secrets
 * never reach the phone (the server masks them), and the phone doesn't ask for them.
 */
data class IndexerRow(
    /** The indexer's id on the server (also in its Torznab address, `/{id}/api`). */
    val id: Long,
    /** The name the owner gave it. */
    val name: String,
    /** "torrent" or "usenet". */
    val protocol: String,
    /** "public", "semi-private" or "private". */
    val privacy: String,
    /** Whether Lantern searches it. */
    val enabled: Boolean,
    /** How many times in a row it failed (0: the last search worked). */
    val failures: Int,
    /** Until when (seconds since 1970) Lantern leaves it alone after repeated failures; 0 when it isn't on hold. */
    val onHoldUntil: Double,
    /** What went wrong the last time, in the server's words ("" when nothing did). */
    val lastError: String,
)

/** How an indexer is doing, with the badge's words and the kit's badge kind. */
enum class Health(
    /** The badge's words. */
    val label: String,
    /** The kit's badge kind ("ok", "warn", "err", "" for muted). */
    val kind: String,
) {
    /** The last search worked. */
    Working("Working", "ok"),

    /** The last search failed; Lantern still tries it. */
    Failing("Failing", "warn"),

    /** It failed again and again, so Lantern waits before trying it again. */
    OnHold("On hold", "err"),

    /** Turned off by the owner. */
    Off("Off", ""),
}

/** One indexer's numbers over the last days (`GET /api/indexers/stats?days=`). */
data class IndexerStats(
    /** Searches (and RSS reads) sent to it. */
    val queries: Int,
    /** Releases grabbed through it. */
    val grabs: Int,
    /** Searches or grabs that failed. */
    val failures: Int,
    /** Its average answer time in milliseconds (0 when there was no search). */
    val avgMs: Int,
)

/** What testing an indexer said (`POST /api/indexers/{id}/test`). */
data class TestResult(
    /** Whether Lantern signed in and searched it. */
    val ok: Boolean,
    /** The server's words. */
    val message: String,
)

/** How many indexers there are and how they are doing, for the home page. */
data class HealthSummary(
    /** Every indexer. */
    val total: Int,
    /** Turned on and working. */
    val working: Int,
    /** Turned on and failing, or on hold. */
    val trouble: Int,
    /** Turned off. */
    val off: Int,
)

/**
 * The indexer rules: reading the server's answers, each indexer's health and the words shown for it. Plain Kotlin,
 * so the unit tests run it on the JVM.
 */
object IndexerRules {
    /** The indexers of `GET /api/indexers` (`{"indexers": [...]}`), in the server's order. */
    fun indexersOf(o: JsonObject?): List<IndexerRow> = o?.arr("indexers").objectsOf().map {
        IndexerRow(
            id = it.long("id"),
            name = it.str("name").ifEmpty { "Indexer ${it.long("id")}" },
            protocol = it.str("protocol"),
            privacy = it.str("privacy"),
            enabled = it.bool("enabled"),
            failures = it.int("failures"),
            onHoldUntil = it.dbl("on_hold_until"),
            lastError = it.str("last_error"),
        )
    }

    /** The numbers of `GET /api/indexers/stats` (`{"stats": [...]}`), by indexer id. */
    fun statsOf(o: JsonObject?): Map<Long, IndexerStats> = o?.arr("stats").objectsOf().associate {
        it.long("id") to IndexerStats(queries = it.int("queries"), grabs = it.int("grabs"), failures = it.int("failures"), avgMs = it.int("avg_ms"))
    }

    /** The answer of a test (`{"ok": bool, "message": "…"}`); words of its own when the server gave none. */
    fun testOf(o: JsonObject?): TestResult {
        val ok = o?.bool("ok") == true
        val message = o?.str("message").orEmpty().ifEmpty { if (ok) "Works" else "The test failed, without saying why" }
        return TestResult(ok, message)
    }

    /** How [i] is doing at [now] (seconds since 1970). */
    fun health(i: IndexerRow, now: Double): Health = when {
        !i.enabled -> Health.Off
        i.onHoldUntil > now -> Health.OnHold
        i.failures > 0 -> Health.Failing
        else -> Health.Working
    }

    /** The line under an indexer's name: what is wrong and what Lantern does about it, or "" when it works. */
    fun healthLine(i: IndexerRow, now: Double): String {
        val why = i.lastError.trim().trimEnd('.')
        val reason = if (why.isEmpty()) "" else ": $why"
        return when (health(i, now)) {
            Health.Working -> ""
            Health.Off -> "Turned off. Turn it on in Lantern's web page to search it again."
            Health.Failing -> "The last ${times(i.failures, "search")} failed$reason. Lantern keeps trying."
            Health.OnHold -> "Paused for ${minutesWords(i.onHoldUntil - now)} after failing again and again$reason."
        }
    }

    /** One line of [s]: "12 searches · 3 grabs · 1 failure · 420 ms"; "No searches in the last 7 days" for none. */
    fun statsLine(s: IndexerStats?, days: Int = 7): String {
        if (s == null || s.queries + s.grabs + s.failures == 0) return "No searches in the last $days days"
        val parts = mutableListOf(count(s.queries, "search", "searches"), count(s.grabs, "grab", "grabs"))
        if (s.failures > 0) parts += count(s.failures, "failure", "failures")
        if (s.avgMs > 0) parts += "${s.avgMs} ms"
        return parts.joinToString(" · ")
    }

    /** The counts of [list] at [now]. */
    fun summary(list: List<IndexerRow>, now: Double): HealthSummary {
        val h = list.map { health(it, now) }
        return HealthSummary(
            total = list.size,
            working = h.count { it == Health.Working },
            trouble = h.count { it == Health.Failing || it == Health.OnHold },
            off = h.count { it == Health.Off },
        )
    }

    /** The home page's line for [s]: whether every indexer works, or how many have trouble. */
    fun summaryLine(s: HealthSummary): String = when {
        s.total == 0 -> "No indexers yet. Add your sites on Lantern's web page."
        s.trouble == 0 && s.working == 0 -> "Every indexer is turned off"
        s.trouble == 0 -> if (s.working == 1) "The indexer works" else "All ${s.working} indexers work"
        else -> "${s.trouble} of ${s.total - s.off} ${if (s.total - s.off == 1) "indexer has" else "indexers have"} trouble"
    }

    private fun times(n: Int, word: String): String = if (n <= 1) word else "$n ${word}es"

    private fun count(n: Int, one: String, many: String): String = "$n ${if (n == 1) one else many}"

    private fun minutesWords(seconds: Double): String {
        val m = Math.ceil(seconds / 60).toInt().coerceAtLeast(1)
        return when {
            m < 60 -> count(m, "minute", "minutes")
            else -> count(Math.ceil(m / 60.0).toInt(), "hour", "hours")
        }
    }
}
