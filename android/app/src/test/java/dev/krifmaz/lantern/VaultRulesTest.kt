package dev.krifmaz.lantern

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The vault rules: reading the server's answers, the reveal request and the words shown. */
class VaultRulesTest {
    private fun json(text: String): JsonObject = Json.parseToJsonElement(text).jsonObject

    private val fields = listOf(
        VaultField("username", "Username", false),
        VaultField("password", "Password", true),
        VaultField("passkey", "Passkey", true),
        VaultField("totp_seed", "2FA seed", true),
    )

    private val item = VaultItem(
        id = "a1",
        kind = "tracker",
        kindLabel = "Tracker account",
        name = "Alpha",
        site = "https://alpha.example",
        plain = mapOf("username" to "ana"),
        secrets = listOf("password", "passkey"),
    )

    /** `GET /api/vault` reads whether it is open, why not, and its fields in order. */
    @Test
    fun statusOf() {
        val s = VaultRules.statusOf(
            json(
                """{"open":false,"locked_because":"The vault key file is missing.","key_source":"file","items":3,
                "fields":[{"key":"username","label":"Username","secret":false},{"key":"password","label":"Password","secret":true}]}""",
            ),
        )
        assertFalse(s.open)
        assertEquals("The vault key file is missing.", s.lockedBecause)
        assertEquals(3, s.items)
        assertEquals(listOf(VaultField("username", "Username", false), VaultField("password", "Password", true)), s.fields)
        assertEquals("The vault key file is missing.", VaultRules.lockedWords(s))
        assertTrue(VaultRules.statusOf(json("""{"open":true,"locked_because":null}""")).open)
        assertTrue(VaultRules.lockedWords(VaultRules.statusOf(null)).startsWith("The vault is locked"))
    }

    /** `GET /api/vault/items` reads plain values and secret names; empty values are left out. */
    @Test
    fun itemsOf() {
        val list = VaultRules.itemsOf(
            json(
                """{"items":[{"id":"a1","kind":"tracker","kind_label":"Tracker account","name":"Alpha","site":"https://alpha.example",
                "fields":{"username":"ana","email":""},"secrets":["password","passkey"],"created_at":1.0}]}""",
            ),
        )
        assertEquals(listOf(item), list)
        assertEquals(emptyList<VaultItem>(), VaultRules.itemsOf(null))
    }

    /** The reveal sends the password; its answer has every value and the 2FA code, when there is one. */
    @Test
    fun reveal() {
        assertEquals(json("""{"password":"pw 1"}"""), VaultRules.revealBody("pw 1"))
        val r = VaultRules.revealedOf(json("""{"id":"a1","fields":{"username":"ana","password":"s3cret","totp_seed":"JBSW"},"totp":{"code":"123456","seconds_left":17}}"""))
        assertEquals("s3cret", r.fields["password"])
        assertEquals(TotpCode("123456", 17), r.totp)
        assertNull(VaultRules.revealedOf(json("""{"fields":{},"totp":null}""")).totp)
        assertEquals("123 456", VaultRules.codeWords("123456"))
        assertEquals("12345678", VaultRules.codeWords("12345678"))
    }

    /** Rows follow the vault's field order; secrets stay hidden until revealed. */
    @Test
    fun rows() {
        val hidden = VaultRules.rows(item, fields, null)
        assertEquals(
            listOf(Triple("Username", "ana", false), Triple("Password", VaultRules.HIDDEN, true), Triple("Passkey", VaultRules.HIDDEN, true)),
            hidden,
        )
        val shown = VaultRules.rows(item, fields, RevealedItem(mapOf("username" to "ana", "password" to "s3cret", "passkey" to "abc"), null))
        assertEquals(listOf("ana", "s3cret", "abc"), shown.map { it.second })
        val odd = item.copy(plain = mapOf("username" to "ana", "region" to "eu"))
        assertEquals("region", VaultRules.rows(odd, fields, null).last().first)
    }

    /** The line names the kind and the saved secrets. */
    @Test
    fun line() {
        assertEquals("Tracker account · Password, Passkey", VaultRules.line(item, fields))
        assertEquals("Tracker account", VaultRules.line(item.copy(secrets = emptyList()), fields))
    }
}
