package dev.krifmaz.lantern

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The download rules: reading the hub's answers, filters and the words shown. */
class DownloadRulesTest {
    private fun json(text: String): JsonObject = Json.parseToJsonElement(text).jsonObject

    private fun grab(state: String, app: Long = 1, error: String = "", attempts: Int = 1, next: Double = 0.0) =
        Grab(1, app, "Sonarr", "Show S01E01", "torrent", state, 0.42, 1503238553, 5_452_595, 180, error, attempts, next, "")

    /** `GET /api/hub/grabs` reads into downloads; progress stays between 0 and 1. */
    @Test
    fun grabsOf() {
        val list = DownloadRules.grabsOf(
            json(
                """{"grabs":[{"id":7,"app_id":2,"app":"Radarr","title":"Dune","protocol":"usenet","state":"downloading","progress":1.5,
                "size":100,"speed":10,"eta":5,"path":"/movies/Dune","files":[],"error":"","attempts":1,"next_try":0,"added":1.0}]}""",
            ),
        )
        assertEquals(1, list.size)
        assertEquals(Grab(7, 2, "Radarr", "Dune", "usenet", "downloading", 1.0, 100, 10, 5, "", 1, 0.0, "/movies/Dune"), list[0])
        assertEquals(emptyList<Grab>(), DownloadRules.grabsOf(null))
    }

    /** `GET /api/hub/apps` reads each app with its counts by state. */
    @Test
    fun appsOf() {
        val a = DownloadRules.appsOf(
            json(
                """{"apps":[{"id":2,"name":"Sonarr","media":"tv","category":"sonarr","folder":"/data/sonarr","import_path":"/downloads",
                "callback_url":"","callback_error":"","source":"native","added":1.0,"last_seen":2.0,"grabs":{"downloading":2,"completed":10,"seeding":1,"failed":1}}]}""",
            ),
        ).single()
        assertEquals("Sonarr", a.name)
        assertEquals(mapOf("downloading" to 2, "completed" to 10, "seeding" to 1, "failed" to 1), a.grabs)
        assertEquals("TV · 2 active · 11 done · 1 failed", DownloadRules.appLine(a))
        assertEquals("No downloads yet", DownloadRules.appLine(a.copy(media = "", grabs = emptyMap())))
    }

    /** Filters keep their states, by app when asked; removed downloads never show. */
    @Test
    fun filter() {
        val list = listOf(grab("queued"), grab("downloading", app = 2), grab("seeding"), grab("completed"), grab("failed"), grab("removed"))
        assertEquals(5, DownloadRules.filter(list, GrabFilter.All).size)
        assertEquals(listOf("queued", "downloading"), DownloadRules.filter(list, GrabFilter.Active).map { it.state })
        assertEquals(listOf("seeding", "completed"), DownloadRules.filter(list, GrabFilter.Done).map { it.state })
        assertEquals(listOf("failed"), DownloadRules.filter(list, GrabFilter.Failed).map { it.state })
        assertEquals(listOf("downloading"), DownloadRules.filter(list, GrabFilter.All, appId = 2).map { it.state })
        assertTrue(DownloadRules.busy(grab("waiting")))
        assertFalse(DownloadRules.busy(grab("seeding")))
    }

    /** The line shows progress, speed and time left while downloading, else the size. */
    @Test
    fun line() {
        assertEquals("Sonarr · 42% of 1.4 GB · 5.2 MB/s · 3 min left", DownloadRules.line(grab("downloading")))
        assertEquals("Sonarr · 1.4 GB", DownloadRules.line(grab("seeding")))
        assertEquals("Done" to "ok", DownloadRules.stateBadge("completed"))
        assertEquals("Failed" to "err", DownloadRules.stateBadge("failed"))
    }

    /** The problem line says what went wrong and what Lantern does next. */
    @Test
    fun problemLine() {
        assertEquals("", DownloadRules.problemLine(grab("downloading"), 0.0))
        assertEquals(
            "qBittorrent didn't answer. Lantern tries again in 2 min.",
            DownloadRules.problemLine(grab("queued", error = "qBittorrent didn't answer.", next = 1120.0), 1000.0),
        )
        assertEquals(
            "Wrong password. Lantern gave up after 5 tries; tap Try again to send it again.",
            DownloadRules.problemLine(grab("failed", error = "Wrong password", attempts = 5), 0.0),
        )
    }

    /** Times read as people say them. */
    @Test
    fun etaWords() {
        assertEquals("40 s", DownloadRules.etaWords(40))
        assertEquals("3 min", DownloadRules.etaWords(200))
        assertEquals("2 h", DownloadRules.etaWords(7200))
        assertEquals("2 h 5 min", DownloadRules.etaWords(7500))
        assertEquals("1 day", DownloadRules.etaWords(90_000))
        assertEquals("3 days", DownloadRules.etaWords(3 * 86_400))
    }
}
