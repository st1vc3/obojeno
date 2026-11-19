import Testing

@testable import Obojeno

struct UpdaterTests {
    @Test(arguments: [
        ("0.2.0", "0.1.0", true),
        ("0.10.0", "0.9.1", true),
        ("1.0", "0.99.99", true),
        ("0.1.1", "0.1", true),
        ("0.1.0", "0.1.0", false),
        ("0.1", "0.1.0", false),
        ("0.1.0", "0.2.0", false),
        ("0.2.0-beta", "0.1.9", true),
    ])
    func comparesVersionsNumerically(newer: String, current: String, expected: Bool) {
        #expect(Updater.isNewer(newer, than: current) == expected)
    }
}
