import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router";
import { AuthGate, Layout, useProjects } from "./components/Layout";
import "./index.css";
import AgentsPage from "./pages/Agents";
import BoardPage from "./pages/Board";
import InboxPage from "./pages/Inbox";
import IssueDrawer from "./pages/IssueDrawer";
import ProjectSettings, { NewProjectPage } from "./pages/ProjectSettings";
import PullPage from "./pages/PullPage";
import { RunDetail, RunsPage } from "./pages/Runs";

const qc = new QueryClient({
  defaultOptions: { queries: { staleTime: 5000, refetchOnWindowFocus: true, retry: 1 } },
});

function Home() {
  const projects = useProjects();
  if (projects.isLoading) return null;
  if (!projects.data?.length) return <Navigate to="/new-project" replace />;
  return <Navigate to={`/p/${projects.data[0].slug}`} replace />;
}

function App() {
  return (
    <Routes>
      <Route path="/" element={<Home />} />
      <Route path="/new-project" element={<Layout><NewProjectPage /></Layout>} />
      <Route path="/p/:slug" element={<Layout><BoardPage /></Layout>}>
        <Route path="issues/:n" element={<IssueDrawer />} />
      </Route>
      <Route path="/p/:slug/pulls/:n" element={<Layout><PullPage /></Layout>} />
      <Route path="/p/:slug/inbox" element={<Layout><InboxPage /></Layout>} />
      <Route path="/p/:slug/settings" element={<Layout><ProjectSettings /></Layout>} />
      <Route path="/runs" element={<Layout><RunsPage /></Layout>} />
      <Route path="/runs/:id" element={<Layout><RunDetail /></Layout>} />
      <Route path="/agents" element={<Layout><AgentsPage /></Layout>} />
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={qc}>
      <BrowserRouter>
        <AuthGate>
          <App />
        </AuthGate>
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
);
