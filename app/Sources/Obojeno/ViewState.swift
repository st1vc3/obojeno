import SwiftUI

/// SwiftUI's `State` property wrapper under a name that is not also a macro.
/// Recent SDKs spell `@State` as a macro whose plugin ships only with Xcode,
/// so this keeps the app buildable with the Command Line Tools alone.
typealias ViewState<Value> = SwiftUI.State<Value>
