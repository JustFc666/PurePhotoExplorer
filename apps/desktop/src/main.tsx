import React from 'react'
import ReactDOM from 'react-dom/client'
import { App } from './App'
import './styles/theme.css'
import './styles/app.css'
import './styles/settings.css'

const root = document.getElementById('root')
if (!root) {
  throw new Error('找不到 #root 挂载点')
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
