# Stream B (Android UI) hand-over

- Project: android/ (Gradle 9.8.1 wrapper, AGP 9.4.1, Kotlin 2.4.21, Compose 1.12.2, miuix 0.9.4). Build:
  JAVA_HOME=.../openjdk-21/Contents/Home ANDROID_HOME=.../android ./gradlew :app:assembleDebug
- Swap the backend in android/app/src/main/kotlin/io/github/souitouiop/iloader/IloaderApp.kt (`core = FakeCore()`).
- Contract types: core/Models.kt, core/IloaderCore.kt. TwoFactorParams is empty; PairEvent/PairState follow
  core/src/usbmux/pairing.rs (udid + Pairing|WaitingForTrust|Paired|Denied|Failed).
- Fake steering: email keywords (see FakeCore KDoc): no2fa, certs, badauth, locked, underage, anisette, maxapps, download...
- Strings: python3 android/tools/convert_locales.py (writes res/values*/strings.xml + tools/locale_keys.json);
  zh-CN gaps in upstream zh_cn.json are filled from tools/zh_cn_extra.json. Hand-written UI-only strings: res/values*/native_strings.xml.
- Placeholders are numbered alphabetically by name (%1$s = first name alphabetically).
- Verified on an arm64 Android 13 emulator (AVD iloader33, installed via sdkmanager) because localhost:5565 refused connections.
