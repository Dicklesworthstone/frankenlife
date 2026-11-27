import SwiftUI

@main
struct FrankenLifeApp: App {
    @State private var engine = FrankenLifeEngine()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ContentView(engine: engine)
                .preferredColorScheme(.dark)
                .statusBarHidden(engine.cinemaMode)
                .onChange(of: scenePhase) { _, newPhase in
                    engine.handleScenePhaseChange(newPhase)
                }
                .onDisappear {
                    engine.shutdown()
                }
        }
    }
}
