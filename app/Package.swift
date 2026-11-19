// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Obojeno",
    platforms: [.macOS(.v26)],
    targets: [
        .executableTarget(name: "Obojeno", path: "Sources/Obojeno"),
        // Swift Testing needs Xcode; the Command Line Tools cannot run it.
        .testTarget(name: "ObojenoTests", dependencies: ["Obojeno"], path: "Tests/ObojenoTests"),
    ]
)
