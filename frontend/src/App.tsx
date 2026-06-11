import { Routes, Route, Navigate } from "react-router-dom";
import { Sidebar } from "./components/Sidebar";

import Dashboard from "./pages/Dashboard";
import Digging from "./pages/Digging";
import Daily from "./pages/Daily";
import TagsPage from "./pages/Tags";
import Lists from "./pages/Lists";
import Tracks from "./pages/Tracks";
import Backpack from "./pages/Backpack";
import Setup from "./pages/Setup";

const Placeholder = ({ name }: { name: string }) => (
  <div data-page={name}>
    <h1>{name}</h1>
  </div>
);

export default function App() {
  return (
    <div className="app-layout">
      <Sidebar />
      <main className="main-content" id="main-content">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/digging" element={<Digging />} />
          <Route path="/daily" element={<Daily />} />
          <Route path="/backpack" element={<Backpack />} />
          <Route path="/tracks" element={<Tracks />} />
          <Route path="/lists" element={<Lists />} />
          <Route path="/tags" element={<TagsPage />} />
          <Route path="/setup" element={<Setup />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}
