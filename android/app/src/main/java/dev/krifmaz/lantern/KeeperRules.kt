package dev.krifmaz.lantern

import dev.krifmaz.kit.compose.core.arr
import dev.krifmaz.kit.compose.core.bool
import dev.krifmaz.kit.compose.core.dbl
import dev.krifmaz.kit.compose.core.formatSize
import dev.krifmaz.kit.compose.core.obj
import dev.krifmaz.kit.compose.core.objectsOf
import dev.krifmaz.kit.compose.core.str
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.Locale

/** One thing that puts an account at risk (the server's sentence, which names the account). */
data class AccountRisk(
    /** Which rule: login, activity, login_expires, cookie_expires, ratio, hnr, unknown_login. */
    val key: String,
    /** True when it breaks the rule now or within a day; false when it is a warning ahead. */
    val danger: Boolean,
    /** What is wrong and what to do. */
    val text: String,
)

/** One tracker or Usenet account as the account keeper sees it (`GET /api/keeper`'s `accounts`). */
data class KeptAccount(
    /** The vault account's id. */
    val id: String,
    /** Its name. */
    val name: String,
    /** Whether it is linked to an indexer Lantern can sign in through. */
    val linked: Boolean,
    /** Whether Lantern signs in on its own to keep it alive. */
    val keepAlive: Boolean,
    /** The ratio (the site's, else uploaded ÷ downloaded); null when not known. */
    val ratio: Double?,
    /** The lowest ratio the site allows (0: no rule). */
    val minRatio: Double,
    /** Uploaded, in bytes; null when not known. */
    val uploaded: Double?,
    /** Downloaded, in bytes; null when not known. */
    val downloaded: Double?,
    /** Total seed time in hours; null when not known. */
    val seedHours: Double?,
    /** Bonus points; null when not known. */
    val bonus: Double?,
    /** Hit-and-runs the site counts now; null when not known. */
    val hnrCount: Int?,
    /** How many hit-and-runs the site allows (0: no limit). */
    val maxHnr: Int,
    /** When the account last signed in (seconds since 1970; 0 for never or not known). */
    val signedInAt: Double,
    /** Why Lantern's last sign-in on its own failed ("" when it worked). */
    val keepAliveError: String,
    /** What puts it at risk, the worst first. */
    val risks: List<AccountRisk>,
)

/** How an account is doing, with the badge's words and the kit's badge kind. */
enum class AccountHealth(
    /** The badge's words. */
    val label: String,
    /** The kit's badge kind. */
    val kind: String,
) {
    /** It breaks a rule now, or within a day. */
    AtRisk("At risk", "err"),

    /** It will break a rule within the warning days. */
    Warning("Warning", "warn"),

    /** Nothing to worry about. */
    Fine("Fine", "ok"),
}

/** The account keeper rules: reading its answers, each account's health and the words shown. Plain Kotlin, unit-tested. */
object KeeperRules {
    /** The accounts of `GET /api/keeper` (`{"accounts": [...], "warn_days": n}`), the worst first, then by name. */
    fun accountsOf(o: JsonObject?): List<KeptAccount> = o?.arr("accounts").objectsOf().map {
        val rules = it.obj("rules")
        val stats = it.obj("stats")
        KeptAccount(
            id = it.str("vault_id"),
            name = it.str("name").ifEmpty { "Account" },
            linked = it["indexer_id"].let { v -> v != null && v !is JsonNull },
            keepAlive = it.bool("keep_alive"),
            ratio = numberOrNull(it, "ratio"),
            minRatio = rules?.dbl("min_ratio") ?: 0.0,
            uploaded = stats?.let { s -> numberOrNull(s, "uploaded") },
            downloaded = stats?.let { s -> numberOrNull(s, "downloaded") },
            seedHours = stats?.let { s -> numberOrNull(s, "seed_hours") },
            bonus = stats?.let { s -> numberOrNull(s, "bonus") },
            hnrCount = stats?.let { s -> numberOrNull(s, "hnr_count")?.toInt() },
            maxHnr = rules?.dbl("max_hnr")?.toInt() ?: 0,
            signedInAt = it.dbl("signed_in_at"),
            keepAliveError = it.str("keep_alive_error"),
            risks = it.arr("risks").objectsOf().map { r -> AccountRisk(r.str("key"), r.str("level") == "danger", r.str("text")) },
        )
    }.sortedWith(compareBy<KeptAccount> { health(it).ordinal }.thenBy { it.name.lowercase() })

    /** How [a] is doing. */
    fun health(a: KeptAccount): AccountHealth = when {
        a.risks.any { it.danger } -> AccountHealth.AtRisk
        a.risks.isNotEmpty() -> AccountHealth.Warning
        else -> AccountHealth.Fine
    }

    /** The numbers of [a] in one line: "Ratio 1.25 (min 0.8) · ↑ 1.4 TB · ↓ 900 GB · 2 of 5 hit-and-runs". */
    fun numbersLine(a: KeptAccount): String {
        val parts = mutableListOf<String>()
        a.ratio?.let { parts += "Ratio " + num(it) + if (a.minRatio > 0) " (min ${num(a.minRatio)})" else "" }
        a.uploaded?.let { parts += "Up " + formatSize(it.toLong()) }
        a.downloaded?.let { parts += "Down " + formatSize(it.toLong()) }
        a.seedHours?.let { parts += "Seeded " + hoursWords(it) }
        a.bonus?.let { parts += num(it) + " bonus" }
        a.hnrCount?.let {
            parts += when {
                a.maxHnr > 0 -> "$it of ${a.maxHnr} hit-and-runs"
                it == 1 -> "1 hit-and-run"
                else -> "$it hit-and-runs"
            }
        }
        return if (parts.isEmpty()) "No numbers yet. Add them on Lantern's web page, or link the account to its indexer." else parts.joinToString(" · ")
    }

    /** The home page's line for [list]: how many accounts are at risk or need a look. */
    fun summaryLine(list: List<KeptAccount>): String {
        if (list.isEmpty()) return "No accounts kept yet. Save your tracker logins in the vault."
        val bad = list.count { health(it) == AccountHealth.AtRisk }
        val warn = list.count { health(it) == AccountHealth.Warning }
        return when {
            bad > 0 && warn > 0 -> "$bad at risk, $warn to look at soon"
            bad > 0 -> if (bad == 1) "1 account is at risk" else "$bad accounts are at risk"
            warn > 0 -> if (warn == 1) "1 account to look at soon" else "$warn accounts to look at soon"
            list.size == 1 -> "The account is fine"
            else -> "All ${list.size} accounts are fine"
        }
    }

    /** The body that says the account signed in on the site today (`PUT /api/keeper/{id}/stats`). */
    fun signedInTodayBody(): JsonObject = buildJsonObject { put("signed_in", true) }

    /** Hours as people say them: "36 h", "12 days". */
    fun hoursWords(h: Double): String = if (h < 48) "${h.toInt()} h" else "${(h / 24).toInt()} days"

    /** A number without a trailing ".0" ("1", "0.85", "1.25"). */
    fun num(x: Double): String = String.format(Locale.US, "%.2f", x).trimEnd('0').trimEnd('.')

    private fun numberOrNull(o: JsonObject, key: String): Double? {
        val v = o[key] ?: return null
        if (v is JsonNull) return null
        return o.dbl(key)
    }
}
