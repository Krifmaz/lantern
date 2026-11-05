package dev.krifmaz.lantern

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/** What `POST /api/auth/login` came to. */
sealed interface LoginResult {
    /** Signed in: [token] goes with every later request as `Authorization: Bearer`. */
    data class Success(val token: String) : LoginResult

    /** The account has two-step sign-in: ask for the authenticator code ([message] is the server's words). */
    data class NeedsCode(val message: String) : LoginResult

    /** Not signed in; [message] is shown in full (the server's `detail`, e.g. "Invalid username or password"). */
    data class Failed(val message: String) : LoginResult
}

/**
 * The phone's sign-in with the kit's rules: the request body, the answer
 * read into a [LoginResult], the phone's name and the words. Plain Kotlin, so the unit tests run it on the JVM;
 * [ServerApi.login] sends it.
 */
object SignInRules {
    /** The page's title ("Sign in to Notes"). */
    fun title(app: String): String = "Sign in to $app"

    /** The line above the code field when the account has two-step sign-in. */
    const val CODE_PROMPT = "Enter the 6-digit code from your authenticator app"

    /** Shown when the sign-in failed without the server's words. */
    const val FAILED = "Sign-in failed"

    /**
     * The body of `POST /api/auth/login`: the trimmed [username], the [password], the authenticator [code] without
     * spaces (left out when empty) and the phone (`app`), which asks the server for a long-lived app token.
     */
    fun body(username: String, password: String, code: String?, deviceId: String, deviceName: String): JsonObject = buildJsonObject {
        put("username", username.trim())
        put("password", password)
        code?.replace(" ", "")?.takeIf { it.isNotEmpty() }?.let { put("code", it) }
        putJsonObject("app") {
            put("device_id", deviceId)
            put("device_name", deviceName)
        }
    }

    /**
     * The answer: a token on success (`{"user": {…}, "token": "…"}`); a 401 with `two_factor` asks for the code;
     * anything else fails with the server's `detail` (or "Sign-in failed (HTTP <code>)").
     */
    fun result(status: Int, text: String): LoginResult {
        val o = try {
            Json.parseToJsonElement(text) as? JsonObject
        } catch (_: Exception) {
            null
        }
        if (status !in 200..299) {
            val detail = o.text("detail").ifEmpty { "$FAILED (HTTP $status)" }
            val twoFactor = (o?.get("two_factor") as? JsonPrimitive)?.booleanOrNull == true
            return if (status == 401 && twoFactor) LoginResult.NeedsCode(detail) else LoginResult.Failed(detail)
        }
        val token = o.text("token")
        return if (token.isNotEmpty()) LoginResult.Success(token) else LoginResult.Failed("The server didn't start a session")
    }

    /** The phone's name as people know it: maker and model ("Google Pixel 8"), the model alone when it names the maker. */
    fun deviceName(maker: String, model: String): String {
        val m = model.ifEmpty { "Android" }
        return if (maker.isNotEmpty() && !m.startsWith(maker, ignoreCase = true)) "${maker.replaceFirstChar { it.uppercase() }} $m" else m
    }

    private fun JsonObject?.text(key: String): String = (this?.get(key) as? JsonPrimitive)?.takeIf { it.isString }?.content.orEmpty()
}
