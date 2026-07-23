import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'

window.addEventListener('error', (e) => {
  document.getElementById('root')!.innerHTML =
    `<pre style="color:red;padding:20px;white-space:pre-wrap">${e.message}\n${e.filename}:${e.lineno}\n${e.error?.stack || ''}</pre>`
})

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)
