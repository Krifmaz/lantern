package dev.krifmaz.lantern

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The search rules: the request, reading the answer and the words of a result. */
class SearchRulesTest {
    /** The words are trimmed and encoded (spaces as %20); nothing is searched for an empty field. */
    @Test
    fun path() {
        assertEquals("/api/search?q=the%20expanse%20s01&type=tvsearch", SearchRules.path("  the expanse s01 ", SearchKind.Tv))
        assertEquals("/api/search?q=AC%2FDC%20%26%20more&type=music", SearchRules.path("AC/DC & more", SearchKind.Music))
        assertEquals("/api/search?q=dune&type=search", SearchRules.path("dune", SearchKind.All))
        assertNull(SearchRules.path("   ", SearchKind.Movies))
    }

    /** `GET /api/search` reads into results and failures; missing seeders read as unknown. */
    @Test
    fun answerOf() {
        val a = SearchRules.answerOf(
            Json.parseToJsonElement(
                """{"results":[
                  {"title":"Dune 2021 1080p","indexer":"Alpha","size":1503238553,"seeders":120,"leechers":3,"protocol":"torrent","ageMinutes":2900.0,
                   "infoUrl":"https://alpha.example/t/1","downloadUrl":"http://lantern:9696/3/download?link=x&sig=y","magnetUrl":null,
                   "indexerFlags":["freeleech"],"categories":[{"id":2040,"name":"Movies/HD"}]},
                  {"title":"Dune.nzb","indexer":"Beta","size":0,"seeders":null,"protocol":"usenet","ageMinutes":30}
                ],"failed":[{"indexer":"Gamma","error":"Wrong password"}]}""",
            ).jsonObject,
        )
        assertEquals(2, a.results.size)
        val r = a.results[0]
        assertEquals("Dune 2021 1080p", r.title)
        assertEquals(120, r.seeders)
        assertEquals("", r.magnetUrl)
        assertEquals(listOf("freeleech"), r.flags)
        assertEquals(listOf("Movies/HD"), r.categories)
        assertEquals(-1, a.results[1].seeders)
        assertEquals(listOf(SearchFailure("Gamma", "Wrong password")), a.failed)
        assertEquals("1.4 GB · 120 seeders · 2d · Alpha", SearchRules.line(r))
        assertEquals("30m · Beta", SearchRules.line(a.results[1]))
        assertEquals("2 results", SearchRules.countLine(a))
        assertEquals("Nothing found", SearchRules.countLine(SearchRules.answerOf(null)))
    }

    /** Ages are short; flags and failures are words. */
    @Test
    fun words() {
        assertEquals("5m", SearchRules.ageWords(5.9))
        assertEquals("3h", SearchRules.ageWords(200.0))
        assertEquals("12d", SearchRules.ageWords(12 * 1440.0))
        assertEquals("2y", SearchRules.ageWords(2 * 525_600.0))
        assertEquals("Double upload", SearchRules.flagWords("doubleupload"))
        assertEquals("", SearchRules.failedLine(emptyList()))
        assertEquals("Gamma didn't answer: Wrong password", SearchRules.failedLine(listOf(SearchFailure("Gamma", "Wrong password"))))
        assertEquals("2 indexers didn't answer: A, B", SearchRules.failedLine(listOf(SearchFailure("A", "x"), SearchFailure("B", "y"))))
    }
}
