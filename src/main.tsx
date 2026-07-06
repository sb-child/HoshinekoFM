import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import './material-web'
import App from './App.tsx'
import { ErrorBoundary } from './components/ErrorBoundary.tsx'
import electronMock from './mocks/electronMock'

// 在 Tauri 迁移阶段，注入 Electron API mock 以便前端能正常打开。
// TODO: 逐步替换为 Tauri invoke 调用后移除此 mock。
if (!window.electron) {
  window.electron = electronMock;
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
)
