/**
 * Generic API request helper.
 * Returns the parsed JSON response body.
 */
export async function apiRequest(baseURL, apiKey, endpoint, options = {}) {
    const response = await fetch(`${baseURL}${endpoint}`, {
        headers: {
            'Authorization': `Bearer ${apiKey}`,
            'Content-Type': 'application/json',
            ...options.headers
        },
        ...options
    });

    const body = await response.json().catch(() => null);

    if (!response.ok) {
        const message = (body && body.message) || `API Error: ${response.status}`;
        throw new Error(message);
    }

    return body;
}
