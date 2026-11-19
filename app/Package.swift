// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Obojeno",
    platforms: [.macOS(.v26)],
    targets: [
        .executableTarget(name: "Obojeno", path: "Sources/Obojeno")
    ]
)
