package dev.krifmaz.lantern

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Test

/** The indexer rules: reading the server's answers, each indexer's health and the words shown for it. */
class IndexerRulesTest {
    private fun json(text: String): JsonObject = Json.parseToJsonElement(text).jsonObject

    private val now = 1_000_000.0

    private fun row(enabled: Boolean = true, failures: Int = 0, hold: Double = 0.0, error: String = "") =
        IndexerRow(1, "Site", "torrent", "private", enabled, failures, hold, error)

    /** `GET /api/indexers` reads into rows; a missing name gets one, a null hold reads as none. */
    @Test
    fun indexersOf() {
        val list = IndexerRules.indexersOf(
            json(
                """{"indexers":[
                  {"id":3,"name":"Alpha","protocol":"torrent","privacy":"private","enabled":true,"failures":0,"on_hold_until":null,"last_error":""},
                  {"id":4,"name":"","protocol":"usenet","privacy":"private","enabled":false,"failures":2,"on_hold_until":1000600.5,"last_error":"Timed out"}
                ],"definitions":540}""",
            ),
        )
        assertEquals(2, list.size)
        assertEquals(IndexerRow(3, "Alpha", "torrent", "private", true, 0, 0.0, ""), list[0])
        assertEquals("Indexer 4", list[1].name)
        assertEquals(1000600.5, list[1].onHoldUntil, 0.0)
        assertEquals(emptyList<IndexerRow>(), IndexerRules.indexersOf(null))
    }

    /** Off wins, then on hold (only while the hold lasts), then failing, else working. */
    @Test
    fun health() {
        assertEquals(Health.Working, IndexerRules.health(row(), now))
        assertEquals(Health.Failing, IndexerRules.health(row(failures = 1), now))
        assertEquals(Health.OnHold, IndexerRules.health(row(failures = 5, hold = now + 60), now))
        assertEquals(Health.Failing, IndexerRules.health(row(failures = 5, hold = now - 1), now))
        assertEquals(Health.Off, IndexerRules.health(row(enabled = false, failures = 5, hold = now + 60), now))
    }

    /** The line says what went wrong and what Lantern does about it. */
    @Test
    fun healthLine() {
        assertEquals("", IndexerRules.healthLine(row(), now))
        assertEquals("The last search failed: The site timed out. Lantern keeps trying.", IndexerRules.healthLine(row(failures = 1, error = "The site timed out."), now))
        assertEquals("The last 3 searches failed. Lantern keeps trying.", IndexerRules.healthLine(row(failures = 3), now))
        val held = row(failures = 6, hold = now + 290, error = "Wrong password")
        assertEquals("Paused for 5 minutes after failing again and again: Wrong password.", IndexerRules.healthLine(held, now))
        assertEquals("Paused for 2 hours after failing again and again.", IndexerRules.healthLine(row(failures = 9, hold = now + 4000), now))
    }

    /** Stats read by id; the line leaves out what is zero. */
    @Test
    fun stats() {
        val s = IndexerRules.statsOf(json("""{"stats":[{"id":3,"name":"Alpha","queries":12,"grabs":1,"failures":0,"avg_ms":420,"results":300},{"id":4,"queries":null}]}"""))
        assertEquals(IndexerStats(12, 1, 0, 420), s[3L])
        assertEquals("12 searches · 1 grab · 420 ms", IndexerRules.statsLine(s[3L]))
        assertEquals("No searches in the last 7 days", IndexerRules.statsLine(s[4L]))
        assertEquals("No searches in the last 7 days", IndexerRules.statsLine(null))
        assertEquals("1 search · 0 grabs · 1 failure", IndexerRules.statsLine(IndexerStats(1, 0, 1, 0)))
    }

    /** The home page's line counts the indexers that have trouble among those turned on. */
    @Test
    fun summary() {
        assertEquals("No indexers yet. Add your sites on Lantern's web page.", IndexerRules.summaryLine(IndexerRules.summary(emptyList(), now)))
        assertEquals("The indexer works", IndexerRules.summaryLine(IndexerRules.summary(listOf(row()), now)))
        assertEquals("All 2 indexers work", IndexerRules.summaryLine(IndexerRules.summary(listOf(row(), row(), row(enabled = false)), now)))
        assertEquals("Every indexer is turned off", IndexerRules.summaryLine(IndexerRules.summary(listOf(row(enabled = false)), now)))
        val mixed = IndexerRules.summary(listOf(row(), row(failures = 1), row(failures = 4, hold = now + 9), row(enabled = false)), now)
        assertEquals(HealthSummary(total = 4, working = 1, trouble = 2, off = 1), mixed)
        assertEquals("2 of 3 indexers have trouble", IndexerRules.summaryLine(mixed))
        assertEquals("1 of 1 indexer has trouble", IndexerRules.summaryLine(IndexerRules.summary(listOf(row(failures = 1)), now)))
    }

    /** A test's answer reads its words, with words of its own when there are none. */
    @Test
    fun testOf() {
        assertEquals(TestResult(true, "Works: 5 results"), IndexerRules.testOf(json("""{"ok":true,"message":"Works: 5 results"}""")))
        assertEquals(TestResult(false, "Wrong password"), IndexerRules.testOf(json("""{"ok":false,"message":"Wrong password"}""")))
        assertEquals(TestResult(false, "The test failed, without saying why"), IndexerRules.testOf(null))
    }
}
