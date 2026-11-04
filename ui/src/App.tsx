import { useCallback, useEffect, useState } from "react";
import { Gallery } from "./components/Gallery";
import { Sidebar, type View } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { Toasts } from "./components/Toasts";
import { api, on } from "./lib/api";
import { StoreProvider, useStore } from "./lib/store";
import type { GameSummary } from "./lib/types";
import { SettingsPage } from "./settings/SettingsPage";

function Shell() {
  const { notify } = useStore();
  const [view, setView] = useState<View>("library");
  const [games, setGames] = useState<GameSummary[]>([]);
  const [game, setGame] = useState<string | null>(null);

  const loadGames = useCallback(() => {
    api.listGames().then(setGames).catch((e) => notify(String(e), true));
  }, [notify]);

  useEffect(() => {
    loadGames();
    const subs = [on("navigate", (v) => v === "settings" && setView("settings")), on("library-changed", loadGames)];
    return () => subs.forEach((p) => p.then((u) => u()));
  }, [loadGames]);

  return (
    <div className="flex h-full">
      <Sidebar view={view} setView={setView} games={games} selectedGame={game} setSelectedGame={setGame} />
      <main className="flex min-w-0 flex-1 flex-col">
        <StatusBar />
        {view === "library" ? <Gallery game={game} onLibraryChanged={loadGames} /> : <SettingsPage />}
      </main>
      <Toasts />
    </div>
  );
}

export default function App() {
  return (
    <StoreProvider>
      <Shell />
    </StoreProvider>
  );
}
