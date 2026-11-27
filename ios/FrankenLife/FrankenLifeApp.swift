import SwiftUI

@main
struct FrankenLifeApp: App {
    @State private var engine = FrankenLifeEngine()

    var body: some Scene {
        WindowGroup {
            ContentView(engine: engine)
                .preferredColorScheme(.dark)
                .statusBarHidden(engine.cinemaMode)
                .onDisappear {
                    engine.shutdown()
                }
        }
    }
}
