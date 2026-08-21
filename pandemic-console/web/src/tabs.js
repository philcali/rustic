/**
 * Tab switching logic for the admin panel.
 */

/**
 * Initialize tab event listeners and set the active tab.
 * @param {string} activeTab - The name of the initially active tab
 * @param {object} loadFns - Map of tab name -> function that loads that tab's data
 */
export function setupTabs(activeTab, loadFns) {
    document.querySelectorAll('.tab-button').forEach(button => {
        button.addEventListener('click', (e) => {
            const tabName = e.target.dataset.tab;
            switchTab(tabName, loadFns);
        });
    });

    // Activate the initial tab
    if (activeTab && loadFns[activeTab]) {
        switchTab(activeTab, loadFns);
    }
}

/**
 * Switch to the given tab and load its data.
 */
export function switchTab(tabName, loadFns) {
    // Remove active class from all tabs and panels
    document.querySelectorAll('.tab-button').forEach(btn => btn.classList.remove('active'));
    document.querySelectorAll('.tab-panel').forEach(panel => panel.classList.remove('active'));

    // Add active class to selected tab and panel
    document.querySelector(`[data-tab="${tabName}"]`).classList.add('active');
    document.getElementById(`${tabName}-tab`).classList.add('active');

    // Load data for the selected tab
    if (tabName !== 'registry' && loadFns[tabName]) {
        loadFns[tabName]();
    }
}
