const { app, BrowserWindow, ipcMain } = require('electron');
const path = require('path');
const net = require('net');
const fs = require('fs');

let mainWindow = null;
let daemonSocket = null;
let requestId = 0;
let pendingRequests = new Map();
let eventSubscribers = [];

function findDaemonPort(projectDir) {
  // Try to read the port file from the data dir
  const home = process.env.HOME || process.env.USERPROFILE;
  const slug = require('crypto')
    .createHash('sha256')
    .update(projectDir.toLowerCase().replace(/\\/g, '/').replace(/\/$/, ''))
    .digest('hex')
    .slice(0, 16);
  
  const portFile = path.join(home, '.local', 'share', 'termforge', slug, 'daemon.port');
  
  try {
    const port = parseInt(fs.readFileSync(portFile, 'utf-8').trim(), 10);
    if (!isNaN(port)) return port;
  } catch (e) {
    // Port file doesn't exist
  }
  return null;
}

function connectToDaemon(port) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ port, host: '127.0.0.1' }, () => {
      resolve(socket);
    });
    socket.on('error', reject);
  });
}

function sendMessage(socket, method, payload) {
  return new Promise((resolve, reject) => {
    const id = String(++requestId);
    const msg = JSON.stringify({ id, method, payload });
    const len = Buffer.alloc(4);
    len.writeUInt32BE(Buffer.byteLength(msg), 0);
    
    pendingRequests.set(id, { resolve, reject });
    socket.write(Buffer.concat([len, Buffer.from(msg)]));
  });
}

function readMessages(socket) {
  let buffer = Buffer.alloc(0);
  
  socket.on('data', (data) => {
    buffer = Buffer.concat([buffer, data]);
    
    while (buffer.length >= 4) {
      const len = buffer.readUInt32BE(0);
      if (buffer.length < 4 + len) break;
      
      const msg = JSON.parse(buffer.slice(4, 4 + len).toString());
      buffer = buffer.slice(4 + len);
      
      if (msg.id && pendingRequests.has(msg.id)) {
        const { resolve, reject } = pendingRequests.get(msg.id);
        pendingRequests.delete(msg.id);
        if (msg.error) {
          reject(new Error(msg.error));
        } else {
          resolve(msg.payload);
        }
      } else if (msg.type) {
        // Event from daemon
        for (const sub of eventSubscribers) {
          sub(msg);
        }
      }
    }
  });
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1400,
    height: 900,
    minWidth: 800,
    minHeight: 600,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
    },
    titleBarStyle: 'hidden',
    backgroundColor: '#0a0a0f',
  });

  const isDev = process.env.NODE_ENV === 'development' ||
    process.argv.includes('--dev') ||
    !fs.existsSync(path.join(__dirname, '../dist/index.html'));

  if (isDev) {
    mainWindow.loadURL('http://localhost:5173');
    mainWindow.webContents.openDevTools({ mode: 'detach' });
    mainWindow.webContents.on('console-message', (event, level, message) => {
      console.log(`[renderer ${level}] ${message}`);
    });
  } else {
    mainWindow.loadFile(path.join(__dirname, '../dist/index.html'));
  }
}

// IPC handlers for renderer
ipcMain.handle('daemon:connect', async (event, projectDir) => {
  try {
    const port = findDaemonPort(projectDir);
    if (!port) {
      return { error: 'Daemon not running. Start with: forge <project-dir>' };
    }
    
    const socket = await connectToDaemon(port);
    daemonSocket = socket;
    readMessages(socket);
    
    // Subscribe to events
    socket.on('close', () => {
      daemonSocket = null;
    });
    
    return { ok: true, port };
  } catch (e) {
    return { error: e.message };
  }
});

ipcMain.handle('daemon:request', async (event, method, payload) => {
  if (!daemonSocket) {
    throw new Error('Not connected to daemon');
  }
  return sendMessage(daemonSocket, method, payload);
});

ipcMain.on('daemon:subscribe', (event) => {
  eventSubscribers.push((msg) => {
    if (mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.webContents.send('daemon:event', msg);
    }
  });
});

app.whenReady().then(createWindow);

app.on('window-all-closed', () => {
  if (daemonSocket) {
    daemonSocket.destroy();
  }
  app.quit();
});

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) {
    createWindow();
  }
});
