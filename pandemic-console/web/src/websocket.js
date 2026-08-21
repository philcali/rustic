/**
 * WebSocket connection management for real-time event streaming.
 */

let currentConnection = null;

/**
 * Set up a WebSocket connection for real-time updates.
 * @param {string} apiBase - Base URL for the API
 * @param {string} apiKey - Authentication token
 * @param {function} onEvent - Callback for received events
 * @param {function} onReconnect - Callback triggered when reconnecting
 */
export function setupWebSocket(apiBase, apiKey, onEvent, onReconnect) {
    if (currentConnection) {
        currentConnection.close();
    }

    if (!apiKey) return null;

    const parsedUrl = new URL(apiBase);
    const wsProtocol = parsedUrl.protocol === 'https' ? 'wss' : 'ws';
    const wsPort = parsedUrl.port ? `:${parsedUrl.port}` : '';
    console.log('Setting up WebSocket connection...');
    const wsUrl = `${wsProtocol}://${parsedUrl.hostname}${wsPort}/api/events/stream?token=${apiKey}`;
    const ws = new WebSocket(wsUrl);

    currentConnection = ws;

    ws.onopen = () => {
        console.log('WebSocket connected for real-time updates');
    };

    ws.onmessage = (event) => {
        try {
            const data = JSON.parse(event.data);
            onEvent(data);
        } catch (error) {
            console.error('Failed to parse WebSocket message:', error);
        }
    };

    ws.onclose = () => {
        console.log('WebSocket disconnected');
        onReconnect();
    };

    ws.onerror = (error) => {
        console.error('WebSocket error:', error);
    };

    return ws;
}
