package dev.krifmaz.lantern

import dev.krifmaz.kit.compose.core.arr
import dev.krifmaz.kit.compose.core.dbl
import dev.krifmaz.kit.compose.core.formatSize
import dev.krifmaz.kit.compose.core.int
import dev.krifmaz.kit.compose.core.long
import dev.krifmaz.kit.compose.core.objectsOf
import dev.krifmaz.kit.compose.core.str
import dev.krifmaz.kit.compose.core.strings
import kotlinx.serialization.json.JsonObject
import java.net.URLEncoder

/** What to search for: every kind, or one (the server's `type`). */
enum class SearchKind(
    /** The choice's words. */
    val label: String,
    /** The server's `type` for it. */
    val type: String,
) {
    /** Any release. */
    All("All", "search"),

    /** TV shows. */
    Tv("TV", "tvsearch"),

    /** Movies. */
    Movies("Movies", "movie"),

    /** Music. */
    Music("Music", "music"),

    /** Books. */
    Books("Books", "book"),
}

/** One release found (`GET /api/search`'s `results`), with what the result list shows. */
data class SearchResult(
    /** The release's name. */
    val title: String,
    /** The indexer that found it. */
    val indexer: String,
    /** Its size in bytes (0 when the site doesn't say). */
    val size: Long,
    /** Seeders (torrents; -1 when the site doesn't say). */
    val seeders: Int,
    /** Leechers (torrents; -1 when the site doesn't say). */
    val leechers: Int,
    /** "torrent" or "usenet". */
    val protocol: String,
    /** How old it is, in minutes. */
    val ageMinutes: Double,
    /** The release's page on the site ("" when there is none). */
    val infoUrl: String,
    /** Lantern's download link for it (signed, so it opens only this release). */
    val downloadUrl: String,
    /** Its magnet link ("" when there is none). */
    val magnetUrl: String,
    /** Freeleech, half leech, double upload. */
    val flags: List<String>,
    /** Its categories' names. */
    val categories: List<String>,
)

/** An indexer that didn't answer a search, and why. */
data class SearchFailure(
    /** The indexer's name. */
    val indexer: String,
    /** The server's words. */
    val error: String,
)

/** A search's answer: what was found and which indexers didn't answer. */
data class SearchAnswer(
    /** The releases, in the server's order. */
    val results: List<SearchResult>,
    /** The indexers that failed. */
    val failed: List<SearchFailure>,
)

/** The search rules: the request, reading the answer and the words of a result. Plain Kotlin, unit-tested. */
object SearchRules {
    /** Shown when the field is empty. */
    const val EMPTY = "Type what you're looking for"

    /** The path that searches every indexer for [words] of [kind]; null when there is nothing to search for. */
    fun path(words: String, kind: SearchKind): String? {
        val q = words.trim()
        if (q.isEmpty()) return null
        return "/api/search?q=" + URLEncoder.encode(q, "UTF-8").replace("+", "%20") + "&type=" + kind.type
    }

    /** The answer of `GET /api/search` (`{"results": [...], "failed": [...]}`). */
    fun answerOf(o: JsonObject?): SearchAnswer = SearchAnswer(
        results = o?.arr("results").objectsOf().map {
            SearchResult(
                title = it.str("title"),
                indexer = it.str("indexer"),
                size = it.long("size"),
                seeders = if (it["seeders"] == null || it.str("seeders").isEmpty()) -1 else it.int("seeders"),
                leechers = if (it["leechers"] == null || it.str("leechers").isEmpty()) -1 else it.int("leechers"),
                protocol = it.str("protocol"),
                ageMinutes = it.dbl("ageMinutes"),
                infoUrl = it.str("infoUrl"),
                downloadUrl = it.str("downloadUrl"),
                magnetUrl = it.str("magnetUrl"),
                flags = it.arr("indexerFlags").strings(),
                categories = it.arr("categories").objectsOf().map { c -> c.str("name") }.filter { n -> n.isNotEmpty() },
            )
        },
        failed = o?.arr("failed").objectsOf().map { SearchFailure(it.str("indexer"), it.str("error")) },
    )

    /** How old a release is, short: "5m", "3h", "12d", "2y". */
    fun ageWords(minutes: Double): String {
        val m = minutes.coerceAtLeast(0.0)
        return when {
            m < 60 -> "${m.toInt()}m"
            m < 1440 -> "${(m / 60).toInt()}h"
            m < 525_600 -> "${(m / 1440).toInt()}d"
            else -> "${(m / 525_600).toInt()}y"
        }
    }

    /** The line under a result's name: "1.4 GB · 120 seeders · 2d · SiteName". */
    fun line(r: SearchResult): String {
        val parts = mutableListOf<String>()
        if (r.size > 0) parts += formatSize(r.size)
        if (r.protocol == "torrent" && r.seeders >= 0) parts += if (r.seeders == 1) "1 seeder" else "${r.seeders} seeders"
        parts += ageWords(r.ageMinutes)
        if (r.indexer.isNotEmpty()) parts += r.indexer
        return parts.joinToString(" · ")
    }

    /** The words of a flag ("freeleech" → "Freeleech"). */
    fun flagWords(flag: String): String = when (flag) {
        "freeleech" -> "Freeleech"
        "halfleech" -> "Half leech"
        "doubleupload" -> "Double upload"
        else -> flag.replaceFirstChar { it.uppercase() }
    }

    /** The line about indexers that didn't answer: "2 indexers didn't answer: A, B"; "" when all did. */
    fun failedLine(failed: List<SearchFailure>): String = when (failed.size) {
        0 -> ""
        1 -> "${failed[0].indexer} didn't answer: ${failed[0].error}"
        else -> "${failed.size} indexers didn't answer: " + failed.joinToString(", ") { it.indexer }
    }

    /** The line above the results: "24 results" / "Nothing found". */
    fun countLine(a: SearchAnswer): String = when (a.results.size) {
        0 -> "Nothing found"
        1 -> "1 result"
        else -> "${a.results.size} results"
    }
}
