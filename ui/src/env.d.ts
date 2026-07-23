declare global {
  interface Window {
    daemon: {
      connect: (projectDir: string) => Promise<{ ok?: boolean; port?: number; error?: string }>;
      request: (method: string, payload?: any) => Promise<any>;
      subscribe: () => void;
      onEvent: (callback: (msg: any) => void) => void;
    };
  }
}

export {};
