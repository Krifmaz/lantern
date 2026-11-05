package dev.krifmaz.lantern

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The account keeper rules: reading its answers, each account's health and the words shown. */
class KeeperRulesTest {
    private fun json(text: String): JsonObject = Json.parseToJsonElement(text).jsonObject

    private val answer = json(
        """{"accounts":[
          {"vault_id":"b","name":"Beta","indexer_id":null,"keep_alive":false,"rules":{"min_ratio":0,"max_hnr":0},
           "stats":{"uploaded":null,"downloaded":null,"ratio":null,"seed_hours":null,"bonus":null,"hnr_count":null},
           "signed_in_at":null,"keep_alive_error":"","ratio":null,"risks":[]},
          {"vault_id":"a","name":"Alpha","indexer_id":3,"keep_alive":true,"rules":{"min_ratio":0.8,"max_hnr":5},
           "stats":{"uploaded":1539316278886.4,"downloaded":966367641600,"ratio":null,"seed_hours":300,"bonus":1200.5,"hnr_count":2},
           "signed_in_at":1000.0,"keep_alive_error":"Wrong password","ratio":1.5929,
           "risks":[{"key":"login","level":"danger","days_left":0.5,"text":"Alpha: sign in on the site today."},
                    {"key":"cookie_expires","level":"warn","days_left":4,"text":"Alpha: the cookie runs out within 4 days."}]},
          {"vault_id":"c","name":"Gamma","indexer_id":null,"keep_alive":false,"rules":{},"stats":{},
           "risks":[{"key":"unknown_login","level":"warn","days_left":null,"text":"Gamma: Lantern doesn't know when you last signed in."}]}
        ],"warn_days":7,"channels":{"ntfy":true}}""",
    )

    /** Accounts read with their numbers and risks, the worst first, then by name. */
    @Test
    fun accountsOf() {
        val list = KeeperRules.accountsOf(answer)
        assertEquals(listOf("Alpha", "Gamma", "Beta"), list.map { it.name })
        val a = list[0]
        assertTrue(a.linked)
        assertTrue(a.keepAlive)
        assertEquals(1.5929, a.ratio!!, 0.0)
        assertEquals(2, a.hnrCount)
        assertEquals("Wrong password", a.keepAliveError)
        assertEquals(AccountRisk("login", true, "Alpha: sign in on the site today."), a.risks[0])
        assertFalse(list[2].linked)
        assertNull(list[2].ratio)
        assertEquals(emptyList<KeptAccount>(), KeeperRules.accountsOf(null))
    }

    /** Danger wins over a warning; no risk is fine. */
    @Test
    fun health() {
        val list = KeeperRules.accountsOf(answer)
        assertEquals(listOf(AccountHealth.AtRisk, AccountHealth.Warning, AccountHealth.Fine), list.map { KeeperRules.health(it) })
    }

    /** The numbers line leaves out what isn't known. */
    @Test
    fun numbersLine() {
        val list = KeeperRules.accountsOf(answer)
        assertEquals("Ratio 1.59 (min 0.8) · Up 1.4 TB · Down 900.0 GB · Seeded 12 days · 1200.5 bonus · 2 of 5 hit-and-runs", KeeperRules.numbersLine(list[0]))
        assertTrue(KeeperRules.numbersLine(list[2]).startsWith("No numbers yet"))
        assertEquals("36 h", KeeperRules.hoursWords(36.0))
        assertEquals("1", KeeperRules.num(1.0))
        assertEquals("0.85", KeeperRules.num(0.85))
    }

    /** The home page's line counts accounts at risk and those to look at. */
    @Test
    fun summaryLine() {
        val list = KeeperRules.accountsOf(answer)
        assertEquals("1 at risk, 1 to look at soon", KeeperRules.summaryLine(list))
        assertEquals("1 account is at risk", KeeperRules.summaryLine(list.take(1)))
        assertEquals("The account is fine", KeeperRules.summaryLine(list.drop(2)))
        assertTrue(KeeperRules.summaryLine(emptyList()).startsWith("No accounts kept yet"))
        assertEquals(json("""{"signed_in":true}"""), KeeperRules.signedInTodayBody())
    }
}
