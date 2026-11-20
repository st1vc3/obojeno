// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Obojeno",
    platforms: [.macOS(.v26)],
    dependencies: [
        // Updates for downloaded installs. Exact: the release workflow signs
        // updates with the same version's tools.
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.1")
    ],
    targets: [
        .executableTarget(
            name: "Obojeno",
            dependencies: [.product(name: "Sparkle", package: "Sparkle")],
            path: "Sources/Obojeno",
            // build-app.sh puts Sparkle.framework in Contents/Frameworks.
            linkerSettings: [.unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"])]
        ),
        // Swift Testing needs Xcode; the Command Line Tools cannot run it.
        .testTarget(name: "ObojenoTests", dependencies: ["Obojeno"], path: "Tests/ObojenoTests"),
    ]
)
