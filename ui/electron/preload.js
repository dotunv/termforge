const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('daemon', {
  connect: (projectDir) => ipcRenderer.invoke('daemon:connect', projectDir),
  request: (method, payload) => ipcRenderer.invoke('daemon:request', method, payload),
  subscribe: () => {
    ipcRenderer.send('daemon:subscribe');
  },
  onEvent: (callback) => {
    ipcRenderer.on('daemon:event', (event, msg) => callback(msg));
  },
});
