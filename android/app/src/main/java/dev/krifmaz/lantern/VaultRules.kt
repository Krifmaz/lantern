package dev.krifmaz.lantern

import dev.krifmaz.kit.compose.core.arr
import dev.krifmaz.kit.compose.core.bool
import dev.krifmaz.kit.compose.core.int
import dev.krifmaz.kit.compose.core.obj
import dev.krifmaz.kit.compose.core.objectsOf
import dev.krifmaz.kit.compose.core.str
import dev.krifmaz.kit.compose.core.strings
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** One field a vault account can have (`GET /api/vault`'s `fields`), in the order the server lists them. */
data class VaultField(
    /** The field's name in the API ("username", "passkey" …). */
    val key: String,
    /** Its words ("Username", "Passkey" …). */
    val label: String,
    /** Whether its value is a secret: listed by name only, shown only after the password is typed again. */
    val secret: Boolean,
)

/** Whether the vault is open, and the fields it offers (`GET /api/vault`). */
data class VaultStatus(
    /** The vault's key is there, so accounts can be read and saved. */
    val open: Boolean,
    /** Why it is locked, in the server's words ("" when it is open). */
    val lockedBecause: String,
    /** How many accounts it holds. */
    val items: Int,
    /** Every field, in order. */
    val fields: List<VaultField>,
)

/** One saved account as listed: its plain fields with their values, its secrets by name only. */
data class VaultItem(
    /** The account's id on the server. */
    val id: String,
    /** "tracker", "usenet_provider", "usenet_indexer" or "other". */
    val kind: String,
    /** The kind's words ("Tracker account" …). */
    val kindLabel: String,
    /** The name the owner gave it. */
    val name: String,
    /** The site's address ("" when none was given). */
    val site: String,
    /** The plain fields and their values (username, email, server …). */
    val plain: Map<String, String>,
    /** The keys of the secrets saved for it (no values). */
    val secrets: List<String>,
)

/** A one-time 2FA code from a saved 2FA seed. */
data class TotpCode(
    /** The code, six digits as a rule. */
    val code: String,
    /** Seconds it stays good for, counted from when the server answered. */
    val secondsLeft: Int,
)

/** One account with every value, after the password was typed again (`POST /api/vault/items/{id}/reveal`). */
data class RevealedItem(
    /** Every field with its value (secrets too). */
    val fields: Map<String, String>,
    /** The 2FA code now, when a 2FA seed is saved; null otherwise. */
    val totp: TotpCode?,
)

/** The vault rules: reading the server's answers, the reveal request and the words shown. Plain Kotlin, unit-tested. */
object VaultRules {
    /** Shown when the password field is empty. */
    const val NO_PASSWORD = "Type the password you sign in to Lantern with"

    /** Shown in place of a secret's value until it is revealed. */
    const val HIDDEN = "••••••"

    /** The answer of `GET /api/vault`. */
    fun statusOf(o: JsonObject?): VaultStatus = VaultStatus(
        open = o?.bool("open") == true,
        lockedBecause = o?.str("locked_because").orEmpty(),
        items = o?.int("items") ?: 0,
        fields = o?.arr("fields").objectsOf().map { VaultField(it.str("key"), it.str("label").ifEmpty { it.str("key") }, it.bool("secret")) },
    )

    /** The accounts of `GET /api/vault/items` (`{"items": [...]}`), in the server's order (by kind, then name). */
    fun itemsOf(o: JsonObject?): List<VaultItem> = o?.arr("items").objectsOf().map {
        VaultItem(
            id = it.str("id"),
            kind = it.str("kind"),
            kindLabel = it.str("kind_label").ifEmpty { "Account" },
            name = it.str("name").ifEmpty { "Account" },
            site = it.str("site"),
            plain = textMap(it.obj("fields")),
            secrets = it.arr("secrets").strings().filter { s -> s.isNotEmpty() },
        )
    }

    /** The body of a reveal: the account's own password, typed again. */
    fun revealBody(password: String): JsonObject = buildJsonObject { put("password", password) }

    /** The answer of a reveal. */
    fun revealedOf(o: JsonObject?): RevealedItem {
        val t = o?.obj("totp")
        val code = t?.str("code").orEmpty()
        return RevealedItem(fields = textMap(o?.obj("fields")), totp = if (code.isEmpty()) null else TotpCode(code, t?.int("seconds_left") ?: 0))
    }

    /**
     * The rows to show for [item], in the vault's field order: each plain field with its value, each saved secret
     * with its value from [revealed] or [HIDDEN]. Fields the vault doesn't know come last, by their key.
     */
    fun rows(item: VaultItem, fields: List<VaultField>, revealed: RevealedItem?): List<Triple<String, String, Boolean>> {
        val known = fields.associateBy { it.key }
        val keys = (fields.map { it.key }.filter { it in item.plain || it in item.secrets }) + (item.plain.keys + item.secrets).filter { it !in known }.distinct()
        return keys.mapNotNull { k ->
            val secret = known[k]?.secret ?: (k in item.secrets)
            val label = known[k]?.label ?: k
            val value = if (secret) revealed?.fields?.get(k) ?: HIDDEN else item.plain[k].orEmpty()
            if (value.isEmpty()) null else Triple(label, value, secret)
        }
    }

    /** The line under an account's name: its kind and what is saved ("Tracker account · Password, Passkey"). */
    fun line(item: VaultItem, fields: List<VaultField>): String {
        val labels = item.secrets.map { k -> fields.firstOrNull { it.key == k }?.label ?: k }
        return if (labels.isEmpty()) item.kindLabel else item.kindLabel + " · " + labels.joinToString(", ")
    }

    /** The words of a locked vault: the server's reason, or ours when it gave none. */
    fun lockedWords(s: VaultStatus): String = s.lockedBecause.ifEmpty { "The vault is locked: Lantern can't find its key. Open Lantern's web page to see why." }

    /** The 2FA code as people read it: "123 456". */
    fun codeWords(code: String): String = if (code.length == 6 && code.all { it.isDigit() }) code.substring(0, 3) + " " + code.substring(3) else code

    private fun textMap(o: JsonObject?): Map<String, String> = o?.keys?.associateWith { o.str(it) }?.filterValues { it.isNotEmpty() }.orEmpty()
}
