import com.android.build.api.variant.ApplicationAndroidComponentsExtension
import org.gradle.process.ExecOperations
import javax.inject.Inject

plugins {
    alias(libs.plugins.androidApplication)
    alias(libs.plugins.composeCompiler)
}

// Rust library (android/bridge), see android/README.md.

val repoRoot: File = rootDir.parentFile
val rustAbis: List<String> =
    (providers.gradleProperty("rustAbis").orNull ?: "arm64-v8a,armeabi-v7a,x86_64").split(",").map { it.trim() }

/** ABI -> (cargo target triple, NDK clang prefix). */
val rustTargets = mapOf(
    "arm64-v8a" to ("aarch64-linux-android" to "aarch64-linux-android"),
    "armeabi-v7a" to ("armv7-linux-androideabi" to "armv7a-linux-androideabi"),
    "x86_64" to ("x86_64-linux-android" to "x86_64-linux-android"),
)

fun cargoEnvironment(): Map<String, String> {
    val cargoHome = System.getenv("CARGO_HOME") ?: "${System.getProperty("user.home")}/.cargo"
    return mapOf("PATH" to "$cargoHome/bin:${System.getenv("PATH") ?: ""}")
}

abstract class BuildRustLibs @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:Input abstract val abis: ListProperty<String>
    @get:Input abstract val release: Property<Boolean>
    @get:Input abstract val minSdk: Property<Int>
    @get:Input abstract val targets: MapProperty<String, String>
    @get:Input abstract val clangPrefixes: MapProperty<String, String>
    @get:Input abstract val env: MapProperty<String, String>
    @get:Internal abstract val workspace: DirectoryProperty
    @get:Internal abstract val ndk: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    init {
        // Cargo does its own incremental builds.
        outputs.upToDateWhen { false }
    }

    @TaskAction
    fun build() {
        val toolchain = ndk.get().asFile.resolve("toolchains/llvm/prebuilt").listFiles()!!.first { it.isDirectory }.resolve("bin")
        val profile = if (release.get()) "release" else "debug"
        for (abi in abis.get()) {
            val triple = targets.get().getValue(abi)
            val clang = toolchain.resolve("${clangPrefixes.get().getValue(abi)}${minSdk.get()}-clang").path
            val key = triple.uppercase().replace('-', '_')
            val cc = triple.replace('-', '_')
            exec.exec {
                workingDir = workspace.get().asFile
                environment(env.get())
                environment("CARGO_TARGET_${key}_LINKER", clang)
                environment("CC_$cc", clang)
                environment("AR_$cc", toolchain.resolve("llvm-ar").path)
                environment("CARGO_PROFILE_DEV_DEBUG", "0")
                environment("CARGO_PROFILE_DEV_STRIP", "debuginfo")
                val args = mutableListOf("cargo", "build", "-p", "iloader-bridge", "--lib", "--target", triple)
                if (release.get()) args += "--release"
                commandLine(args)
            }
            val built = workspace.get().asFile.resolve("target/$triple/$profile/libiloader_bridge.so")
            val dest = outputDir.get().asFile.resolve(abi).apply { mkdirs() }
            built.copyTo(dest.resolve(built.name), overwrite = true)
        }
    }
}

abstract class GenerateBindings @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:Input abstract val env: MapProperty<String, String>
    @get:Internal abstract val workspace: DirectoryProperty
    @get:InputDirectory abstract val libs: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val lib = libs.get().asFile.walkTopDown().first { it.name == "libiloader_bridge.so" }
        outputDir.get().asFile.deleteRecursively()
        exec.exec {
            workingDir = workspace.get().asFile
            environment(env.get())
            commandLine(
                "cargo", "run", "-q", "-p", "iloader-bridge", "--features", "cli", "--bin", "uniffi-bindgen", "--",
                "generate", "--library", lib.path, "--language", "kotlin", "--no-format",
                "--out-dir", outputDir.get().asFile.path,
            )
        }
    }
}

android {
    namespace = "io.github.souitouiop.iloader"
    compileSdk {
        version = release(37) { minorApiLevel = 0 }
    }
    buildToolsVersion = "37.0.0"

    defaultConfig {
        applicationId = "io.github.souitouiop.iloader"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"

        // -PfakeCore=true runs the UI against FakeCore instead of the Rust library.
        buildConfigField("boolean", "USE_FAKE_CORE", (providers.gradleProperty("fakeCore").orNull == "true").toString())
    }

    ndkVersion = "28.2.13676358"

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    buildFeatures {
        buildConfig = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

kotlin {
    jvmToolchain(21)
}

androidComponents {
    onVariants { variant ->
        val name = variant.name.replaceFirstChar { it.uppercase() }
        val libs = tasks.register<BuildRustLibs>("buildRustLibs$name") {
            abis.set(rustAbis)
            release.set(variant.buildType == "release")
            minSdk.set(26)
            targets.set(rustTargets.mapValues { it.value.first })
            clangPrefixes.set(rustTargets.mapValues { it.value.second })
            env.set(cargoEnvironment())
            workspace.set(repoRoot)
            ndk.set(sdkComponents.ndkDirectory)
        }
        variant.sources.jniLibs?.addGeneratedSourceDirectory(libs, BuildRustLibs::outputDir)
        val bindings = tasks.register<GenerateBindings>("generateBindings$name") {
            env.set(cargoEnvironment())
            workspace.set(repoRoot)
            this.libs.set(libs.flatMap { it.outputDir })
        }
        variant.sources.kotlin?.addGeneratedSourceDirectory(bindings, GenerateBindings::outputDir)
    }
}

dependencies {
    implementation("${libs.jna.get()}@aar")

    implementation(libs.compose.foundation)
    implementation(libs.miuix.ui)
    implementation(libs.miuix.preference)
    implementation(libs.miuix.blur)
    implementation(libs.miuix.icons)
    implementation(libs.androidx.activity)
    implementation(libs.lifecycle.viewmodel.compose)
    implementation(libs.lifecycle.runtime.compose)
    implementation(libs.datastore.preferences)
    implementation(libs.coroutines.android)

    testImplementation(libs.junit)
    testImplementation(kotlin("test"))
    testImplementation(libs.coroutines.test)
}
