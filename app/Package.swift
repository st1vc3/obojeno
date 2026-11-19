// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Obojeno",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(name: "Obojeno", path: "Sources/Obojeno")
    ]
)
