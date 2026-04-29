// tag-categories.js - Tag Categories page functionality
document.addEventListener("DOMContentLoaded", function () {
  // DOM elements
  const searchInput = document.getElementById("search-input");
  const searchButton = document.getElementById("search-button");
  const errorMessage = document.getElementById("error-message");
  const tagCategoriesContent = document.getElementById("tag-categories-content");
  const tagCategoriesTableContainer = document.getElementById(
    "tag-categories-table-container",
  );
  const tagCategoriesTableBody = document.getElementById("tag-categories-table-body");
  const tagCategoriesCount = document.getElementById("tag-categories-count");
  const noTagCategories = document.getElementById("no-tag-categories");

  // State
  let allTagCategories = [];
  let filteredTagCategories = [];

  // API endpoint
  const API_BASE = "http://localhost:3000/api";

  // Show error message
  function showError(message) {
    errorMessage.textContent = message;
    errorMessage.style.display = "block";
    setTimeout(() => {
      errorMessage.style.display = "none";
    }, 5000);
  }

  // Fetch tag categories from API
  async function fetchTagCategories() {
    try {
      const response = await fetch(`${API_BASE}/tag-categories`);
      if (!response.ok) {
        throw new Error(`HTTP error! status: ${response.status}`);
      }
      const result = await response.json();
      return result.data || [];
    } catch (error) {
      console.error("Error fetching tag categories:", error);
      showError(`Failed to load tag categories: ${error.message}`);
      return [];
    }
  }

  // Filter tag categories based on search term
  function filterTagCategories(searchTerm) {
    if (!searchTerm.trim()) {
      return allTagCategories;
    }

    const term = searchTerm.toLowerCase();
    return allTagCategories.filter((category) => {
      return (
        (category.name && category.name.toLowerCase().includes(term)) ||
        (category.prefix && category.prefix.toLowerCase().includes(term)) ||
        (category.icon && category.icon.toLowerCase().includes(term))
      );
    });
  }

  // Format date from timestamp
  function formatDate(timestamp) {
    if (!timestamp) return "N/A";
    const date = new Date(timestamp * 1000);
    return date.toLocaleDateString();
  }

  // Render tag categories table
  function renderTagCategories(categories) {
    // Update count
    tagCategoriesCount.textContent = categories.length;

    // Clear table
    tagCategoriesTableBody.innerHTML = "";

    if (categories.length === 0) {
      tagCategoriesTableContainer.style.display = "none";
      noTagCategories.style.display = "block";
      return;
    }

    // Show table and hide "no results" message
    tagCategoriesTableContainer.style.display = "block";
    noTagCategories.style.display = "none";

    // Sort categories by sort_order then name
    const sortedCategories = [...categories].sort((a, b) => {
      if (a.sort_order !== b.sort_order) {
        return (a.sort_order || 0) - (b.sort_order || 0);
      }
      return (a.name || "").localeCompare(b.name || "");
    });

    // Add rows
    sortedCategories.forEach((category) => {
      const row = document.createElement("tr");
      row.style.borderBottom = "1px solid #334155";

      // ID cell
      const idCell = document.createElement("td");
      idCell.textContent = category.id;
      idCell.style.padding = "12px 16px";

      // Name cell
      const nameCell = document.createElement("td");
      nameCell.textContent = category.name || "N/A";
      nameCell.style.padding = "12px 16px";

      // Prefix cell
      const prefixCell = document.createElement("td");
      prefixCell.textContent = category.prefix || "N/A";
      prefixCell.style.padding = "12px 16px";

      // Icon cell
      const iconCell = document.createElement("td");
      if (category.icon) {
        iconCell.innerHTML = `<i class="${category.icon}" title="${category.icon}"></i> ${category.icon}`;
      } else {
        iconCell.textContent = "N/A";
      }
      iconCell.style.padding = "12px 16px";

      // Default cell
      const defaultCell = document.createElement("td");
      if (category.is_default) {
        defaultCell.innerHTML =
          '<span style="color: #10b981; font-weight: 600">✓ Default</span>';
      } else {
        defaultCell.textContent = "No";
      }
      defaultCell.style.padding = "12px 16px";

      // Sort Order cell
      const sortOrderCell = document.createElement("td");
      sortOrderCell.textContent = category.sort_order || 0;
      sortOrderCell.style.padding = "12px 16px";

      // Append cells to row
      row.appendChild(idCell);
      row.appendChild(nameCell);
      row.appendChild(prefixCell);
      row.appendChild(iconCell);
      row.appendChild(defaultCell);
      row.appendChild(sortOrderCell);

      tagCategoriesTableBody.appendChild(row);
    });
  }

  // Perform search
  function performSearch() {
    const searchTerm = searchInput.value.trim();
    filteredTagCategories = filterTagCategories(searchTerm);
    renderTagCategories(filteredTagCategories);
  }

  // Load tag categories
  async function loadTagCategories() {
    // Show loading state
    tagCategoriesContent.innerHTML = `
            <div class="loading">
                <div class="loading-spinner"></div>
                <p>Loading tag categories...</p>
            </div>
        `;

    // Fetch data
    const categories = await fetchTagCategories();

    // Update state
    allTagCategories = categories;
    filteredTagCategories = categories;

    // Hide loading, show content
    tagCategoriesContent.style.display = "none";

    // Render data
    renderTagCategories(filteredTagCategories);
  }

  // Event listeners
  searchButton.addEventListener("click", performSearch);

  searchInput.addEventListener("keyup", function (event) {
    if (event.key === "Enter") {
      performSearch();
    }
  });

  // Initial load
  loadTagCategories();
});
