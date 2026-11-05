# R8 rules for Lantern. The libraries bring their own (kit-compose keeps its serializers; OkHttp, Coil and
# kotlinx.serialization ship theirs). Add rules here only for what the app itself reflects on.

# OkHttp's optional platform integrations
-dontwarn okhttp3.internal.platform.**
-dontwarn org.conscrypt.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**
