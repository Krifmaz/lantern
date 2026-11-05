package dev.krifmaz.lantern

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import dev.krifmaz.kit.compose.KitApp
import dev.krifmaz.kit.compose.core.ApiException
import dev.krifmaz.kit.compose.core.KitApi
import dev.krifmaz.kit.compose.core.bool
import dev.krifmaz.kit.compose.core.parseJson
import dev.krifmaz.kit.compose.core.parseJsonObject
import dev.krifmaz.kit.compose.core.str
import dev.krifmaz.kit.compose.whatsnew.Changelog
import dev.krifmaz.kit.compose.whatsnew.ChangelogBackend
import dev.krifmaz.kit.compose.whatsnew.ChangelogNew
import dev.krifmaz.kit.compose.whatsnew.WhatsNewRules
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.io.IOException
import java.util.UUID
import java.util.concurrent.TimeUnit

/**
 * Which server the app talks to and who is signed in, kept in the preferences file "server": the base URL ("" until
 * the first connect), the app token from signing in (sent as `Authorization: Bearer`) and
 * this phone's id (sent with the sign-in and with crash reports).
 */
class ServerSettings(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    private val baseFlow = MutableStateFlow(prefs.getString(K_BASE, "").orEmpty())
    private val signedInFlow = MutableStateFlow(prefs.getString(K_TOKEN, "").orEmpty().isNotEmpty())

    /** The server's base URL ("https://files.example.com"); "" while the app isn't connected. */
    val base: StateFlow<String> = baseFlow.asStateFlow()

    /** The last server connected to, suggested when the app asks again ("" before the first connect). */
    val last: String get() = prefs.getString(K_LAST, "").orEmpty()

    /** Whether someone is signed in (there is an app token). */
    val signedIn: StateFlow<Boolean> = signedInFlow.asStateFlow()

    /** The app token sent with every request to the server; "" while nobody is signed in. */
    val token: String get() = prefs.getString(K_TOKEN, "").orEmpty()

    /** This phone's id (made once and kept), sent with the sign-in and with crash reports as `device_id`. */
    val deviceId: String
        get() = prefs.getString(K_DEVICE, null) ?: UUID.randomUUID().toString().also { prefs.edit().putString(K_DEVICE, it).apply() }

    /** Uses the server at [base] (already checked) from now on; someone signs in to it next. */
    fun connect(base: String) {
        prefs.edit().putString(K_BASE, base).putString(K_LAST, base).remove(K_TOKEN).apply()
        signedInFlow.value = false
        baseFlow.value = base
    }

    /** Keeps the app token of a sign-in. */
    fun signIn(token: String) {
        prefs.edit().putString(K_TOKEN, token).apply()
        signedInFlow.value = true
    }

    /** Forgets the token (signed out, or the server no longer takes it): the app asks to sign in again. */
    fun signOut() {
        prefs.edit().remove(K_TOKEN).apply()
        signedInFlow.value = false
    }

    /** Forgets the server and the token, so the app asks for them again (the address stays as a suggestion). */
    fun forget() {
        prefs.edit().remove(K_BASE).remove(K_TOKEN).apply()
        signedInFlow.value = false
        baseFlow.value = ""
    }

    private companion object {
        const val PREFS = "server"
        const val K_BASE = "base"
        const val K_LAST = "last"
        const val K_TOKEN = "token"
        const val K_DEVICE = "device_id"
    }
}

/** Who the signed-in account is (`GET /api/auth/me`): an admin sees every bug report and the server's settings. */
data class Me(
    /** May use the admin pages (Diagnostics, Server settings, every bug report). */
    val admin: Boolean,
    /** The server's owner: may also reveal saved secrets in the settings forms. */
    val owner: Boolean,
) {
    /** Reading the answer. */
    companion object {
        /** The account from `GET /api/auth/me` (`is_admin`, or a `role` of admin or owner). */
        fun of(o: JsonObject): Me {
            val role = o.str("role")
            return Me(admin = o.bool("is_admin") || role == "admin" || role == "owner", owner = role == "owner")
        }
    }
}

/**
 * The app's server API, as the kit's parts need it ([KitApi]: bug reports, crash logs, settings forms, diagnostics),
 * over one OkHttp client that adds the User-Agent and, for the server's own address only, the app token. Every
 * call blocks (run it on Dispatchers.IO); a failed call throws [ApiException] with the server's `detail`, or an
 * [IOException] when the server can't be reached.
 */
class ServerApi(private val settings: ServerSettings) {
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    /** The client every request goes through (images, uploads and the offline check too). */
    val http: OkHttpClient =
        OkHttpClient
            .Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(30, TimeUnit.SECONDS)
            .addInterceptor { chain ->
                val r = chain.request()
                val b = r.newBuilder().header("User-Agent", KitApp.userAgent)
                val home = settings.base.value.toHttpUrlOrNull()
                val token = settings.token
                // the token goes only to the server it was given for
                val signed = token.isNotEmpty() && home != null && r.url.host == home.host && r.url.port == home.port
                if (signed) b.header("Authorization", "Bearer $token")
                val res = chain.proceed(b.build())
                // the server no longer takes the token (signed out elsewhere, expired): back to the sign-in
                if (signed && res.code == 401 && settings.token == token) settings.signOut()
                res
            }.build()

    /** The [KitApi] the kit's parts take. */
    val kit: KitApi =
        object : KitApi {
            override fun url(path: String): String = this@ServerApi.url(path)

            override fun get(path: String): JsonElement? = call("GET", path, null)

            override fun post(path: String, body: JsonElement): JsonElement? = call("POST", path, body)

            override fun rawGet(path: String): Response = http.newCall(Request.Builder().url(need(path)).build()).execute()
        }

    /** The full URL of a server [path] ("/api/…"); "" while the app isn't connected. */
    fun url(path: String): String = settings.base.value.let { if (it.isEmpty()) "" else it + path }

    /** GET [path]; the JSON answer (null for an empty one). */
    fun get(path: String): JsonElement? = call("GET", path, null)

    /** GET [path], which must answer a JSON object. */
    fun getObject(path: String): JsonObject = get(path) as? JsonObject ?: throw IOException(UNEXPECTED)

    /** POST [body] as JSON to [path]. */
    fun post(path: String, body: JsonElement): JsonElement? = call("POST", path, body)

    /** PUT [body] as JSON to [path]. */
    fun put(path: String, body: JsonElement): JsonElement? = call("PUT", path, body)

    /** DELETE [path]. */
    fun delete(path: String): JsonElement? = call("DELETE", path, null)

    /** Who is signed in (`GET /api/auth/me`). */
    fun me(): Me = Me.of(getObject("/api/auth/me"))

    /**
     * Finds the server for what was typed : tries each [ServerAddress.candidates] address's
     * `/api/health` and gives the first that answers as a server, in OkHttp's form. Throws with the words to show.
     */
    fun probe(input: String): String {
        val candidates = ServerAddress.candidates(input)
        if (candidates.isEmpty()) throw IOException(ServerAddress.EMPTY)
        var last: Exception? = null
        for (c in candidates) {
            val norm = ServerAddress.normalise(c)
            if (norm == null) {
                last = IOException(ServerAddress.INVALID)
                continue
            }
            try {
                val o = http.newCall(Request.Builder().url("$norm/api/health").build()).execute().use { parseJsonObject(it.body.string()) }
                if (o != null && ServerAddress.isServer(o.bool("ok"), o.str("version"))) return norm
                last = IOException(ServerAddress.notServer(KitApp.name))
            } catch (e: Exception) {
                last = e
            }
        }
        throw IOException(last?.message ?: ServerAddress.UNREACHABLE)
    }

    /**
     * Signs in (`POST /api/auth/login`, asking for an app token for this phone) and keeps the token when it worked.
     * Throws an [IOException] when the server can't be reached.
     */
    fun login(username: String, password: String, code: String?): LoginResult {
        val name = SignInRules.deviceName(Build.MANUFACTURER.orEmpty(), Build.MODEL.orEmpty())
        val body = SignInRules.body(username, password, code, settings.deviceId, name)
        val req = Request.Builder().url(need("/api/auth/login")).header("Accept", "application/json").post(body.toString().toRequestBody(jsonType)).build()
        val result = http.newCall(req).execute().use { SignInRules.result(it.code, it.body.string()) }
        if (result is LoginResult.Success) settings.signIn(result.token)
        return result
    }

    /** Signs out: tells the server (`POST /api/auth/logout`, when it can be reached) and forgets the token. */
    fun logout() {
        try {
            post("/api/auth/logout", JsonObject(emptyMap()))
        } catch (_: IOException) {
            // signed out on the phone anyway
        }
        settings.signOut()
    }

    private fun need(path: String): String = url(path).ifEmpty { throw IOException(NOT_CONNECTED) }

    private fun call(method: String, path: String, body: JsonElement?): JsonElement? {
        val payload = body?.toString()?.toRequestBody(jsonType) ?: if (method == "GET" || method == "DELETE") null else "".toRequestBody(jsonType)
        val req = Request.Builder().url(need(path)).header("Accept", "application/json").method(method, payload).build()
        return http.newCall(req).execute().use { res ->
            val text = res.body.string()
            if (!res.isSuccessful) {
                val o = parseJsonObject(text)
                throw ApiException(res.code, o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}", o)
            }
            parseJson(text)
        }
    }

    private companion object {
        const val UNEXPECTED = "Unexpected reply from the server"
        const val NOT_CONNECTED = "Not connected to a server yet"
    }
}

/** What's new from the app's server (contracts/changelog.md), for the kit's popup and changelog screen. */
class ServerChangelog(private val api: ServerApi) : ChangelogBackend {
    /** `GET /api/changelog/new`. */
    override fun new(): ChangelogNew = WhatsNewRules.newOf(api.getObject("/api/changelog/new"))

    /** `POST /api/changelog/seen`. */
    override fun markSeen() {
        api.post("/api/changelog/seen", JsonObject(emptyMap()))
    }

    /** `GET /api/changelog`. */
    override fun all(): Changelog = WhatsNewRules.changelogOf(api.getObject("/api/changelog"))

    /** `PUT /api/auth/prefs` with `{"whats_new_popup": on}`. */
    override fun setPopup(on: Boolean) {
        api.put("/api/auth/prefs", buildJsonObject { put("whats_new_popup", on) })
    }
}

/** Whether the phone has a network that reaches the internet (offline mode's reason when nothing answers). */
fun phoneOnline(context: Context): Boolean {
    val cm = context.getSystemService(ConnectivityManager::class.java) ?: return true
    val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
    return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
}
