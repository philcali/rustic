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

    if (!response.ok) {
        throw new Error(`API Error: ${response.status}`);
    }

    return response.json();
}
